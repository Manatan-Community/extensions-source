// Run against a dedicated test app after installing the two signed packages.
// node tools/test-novel-sources.mjs http://127.0.0.1:<task-specific-port>
import assert from 'node:assert/strict';
const base = process.argv[2];
assert.match(base ?? '', /^http:\/\/127\.0\.0\.1:\d+$/);
async function request(path, body) {
  const response = await fetch(base + path, body ? {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(body)} : {});
  assert.equal(response.status, 200, `${path}: ${await response.clone().text()}`);
  return response.json();
}
const sources = await request('/api/v1/novel/source/list');
for (const [name, query, title, largeQuery] of [
  ['Shanghai Fantasy', 'Apocalypse: I Can Enhance Everything', 'Apocalypse: I Can Enhance Everything', 'The Strange Realm Descends'],
  ['Second Life Translations', '100 Reasons to Kill My Husband', '100 Reasons to Kill My Husband', null],
]) {
  const source = sources.find(s => s.name === name);
  assert.ok(source, `Missing installed source: ${name}`);
  const root = `/api/v1/novel/source/${source.id}`;
  const catalog = await request(`${root}/popular?page=1`);
  assert.ok(catalog.entries.length > 0, `${name} catalog is empty`);
  assert.ok(catalog.hasNextPage, `${name} catalog pagination missing`);
  const page2 = await request(`${root}/popular?page=2`);
  assert.ok(page2.entries.length > 0);
  assert.notEqual(catalog.entries[0].key, page2.entries[0].key);
  const search = await request(`${root}/search?query=${encodeURIComponent(query)}&page=1`);
  const item = search.entries.find(e => e.title === title);
  assert.ok(item, `${name} search did not return requested novel`);
  const details = await request(`${root}/details`, {item});
  assert.equal(details.title, title);
  assert.ok(details.description?.length > 40);
  assert.ok(details.cover);
  const cover = await fetch(base + details.cover);
  assert.equal(cover.status, 200);
  assert.match(cover.headers.get('content-type') ?? '', /^image\//);
  assert.ok((await cover.arrayBuffer()).byteLength > 100, 'Cover must contain actual image bytes');
  const chapters = await request(`${root}/chapters/page?page=1`, {item: details});
  assert.ok(chapters.entries.length > 0);
  const readable = chapters.entries.filter(c => !c.isLocked);
  assert.ok(readable.length > 0);
  const chapter = readable.at(-1);
  const text = await request(`${root}/text`, {item: details, chapter});
  assert.ok(text.html?.length > 1000);
  assert.ok(!/<(?:script|iframe|form)\b|adsbygoogle|code-block/.test(text.html));
  if (name === 'Second Life Translations') {
    assert.equal(chapter.chapterNumber, 1);
    assert.ok(chapters.entries.length > 29);
    const middle = readable[Math.floor(readable.length / 2)];
    const middleText = await request(`${root}/text`, {item: details, chapter: middle});
    assert.ok(middleText.html?.length > 1000);
  }
  if (largeQuery) {
    const search2 = await request(`${root}/search?query=${encodeURIComponent(largeQuery)}&page=1`);
    assert.ok(search2.entries.length);
    const large = await request(`${root}/details`, {item: search2.entries[0]});
    const newest = await request(`${root}/chapters/page?page=1`, {item: large});
    assert.ok(newest.hasNextPage, 'Large Shanghai novel must exercise chapter pagination');
    const older = await request(`${root}/chapters/page?page=2`, {item: large});
    assert.ok(older.entries.length);
    assert.ok(newest.entries[0].chapterNumber > older.entries[0].chapterNumber);
    assert.ok(newest.entries.some(c => c.isLocked), 'Paid chapter metadata must be retained');
    const last = await request(`${root}/chapters/page?page=${newest.pageCount}`, {item: large});
    const firstChapter = last.entries.filter(c => !c.isLocked).at(-1);
    assert.ok(firstChapter);
    const largeText = await request(`${root}/text`, {item: large, chapter: firstChapter});
    assert.ok(largeText.html?.length > 1000);
  }
  console.log(`${name}: catalog pages 1/2, search, details, cover, chapter order, pagination and reader text PASS (${chapters.entries.length} chapters, ${text.html.length} HTML characters)`);
}
