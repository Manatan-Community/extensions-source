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
  assert.ok(source.iconUrl, `${name} must expose its bundled logo`);
  const icon = await fetch(base + source.iconUrl);
  assert.equal(icon.status, 200);
  assert.match(icon.headers.get('content-type') ?? '', /^image\/png/);
  const iconBytes = new Uint8Array(await icon.arrayBuffer());
  assert.deepEqual([...iconBytes.slice(0, 8)], [137, 80, 78, 71, 13, 10, 26, 10]);
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
    const filters = await request(`${root}/filters`);
    const language = filters.find(f => f.id === 'originalLanguage');
    assert.ok(language);
    assert.match(language.name, /translated to English/);
    assert.deepEqual(language.options.map(o => o.value), ['', 'Chinese', 'Japanese']);
    const filtered = (language, page = 1, query = '') => request(`${root}/search?${new URLSearchParams({query, page: String(page), filters: JSON.stringify({originalLanguage: language})})}`);
    const japanese = await filtered('Japanese');
    const japaneseListing = await request(`${root}/listing/popular?${new URLSearchParams({page: '1', filters: JSON.stringify({originalLanguage: 'Japanese'})})}`);
    assert.deepEqual(japaneseListing.entries.map(e => e.key), japanese.entries.map(e => e.key));
    assert.ok(japanese.entries.length > 0);
    assert.ok(japanese.entries.every(e => e.language === 'en' && e.extra.originalLanguage === 'Japanese'));
    const japaneseItem = japanese.entries.find(e => e.title === 'Serving Meals In Another World');
    assert.ok(japaneseItem);
    const japaneseDetails = await request(`${root}/details`, {item: japaneseItem});
    assert.equal(japaneseDetails.extra.originalLanguage, 'Japanese');
    const japaneseChapters = await request(`${root}/chapters/page?page=1`, {item: japaneseDetails});
    const japaneseText = await request(`${root}/text`, {item: japaneseDetails, chapter: japaneseChapters.entries.at(-1)});
    assert.ok(japaneseText.html?.length > 1000);
    const searchedJapanese = await filtered('Japanese', 1, 'Serving Meals');
    assert.ok(searchedJapanese.entries.some(e => e.key === japaneseItem.key));
    const chinese = await filtered('Chinese');
    const chinesePage2 = await filtered('Chinese', 2);
    assert.ok(chinese.hasNextPage && chinesePage2.entries.length > 0);
    assert.notEqual(chinese.entries[0].key, chinesePage2.entries[0].key);
    assert.ok(chinese.entries.every(e => e.language === 'en' && e.extra.originalLanguage === 'Chinese'));
    assert.ok(!chinese.entries.some(e => e.key === japaneseItem.key));
    console.log('Second Life: original-language filters, Chinese pagination, Japanese search and English chapter text PASS');
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
