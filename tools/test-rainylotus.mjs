// Signed-package guest checks with real HTTP through a production Wasmtime host.
// The host must support GET and the public ac_novel_chapter_page POST action.
// Usage: node tools/test-rainylotus.mjs <runtime-invoker> [--reading-only]
// Interactive sign-in and purchased chapters require a separate account test.
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {resolve} from 'node:path';

const binary = process.argv[2];
const readingOnly = process.argv.includes('--reading-only');
assert.ok(binary, 'Pass a production Wasmtime invoker with an HTTP host bridge');
const pkg = resolve('dist/packages/novel/en/rainylotus.manatan2');
function call(operation, request) {
  const name = operation.startsWith('auth.') || operation === 'filters' ? operation : `novel.${operation}`;
  return JSON.parse(execFileSync(resolve(binary), [pkg, name, JSON.stringify(request)], {
    encoding: 'utf8', timeout: 120000, maxBuffer: 16 * 1024 * 1024, stdio: ['ignore', 'pipe', 'pipe'],
  }));
}
function slim(item) {
  return {key: item.key, url: item.url, title: item.title};
}
function plain(s) {
  return s.replace(/<[^>]*>/g, ' ').replace(/&amp;/g, '&').replace(/&quot;/g, '"')
    .replace(/&#0?39;|&apos;/g, "'").replace(/&nbsp;/g, ' ').replace(/\s+/g, ' ').trim();
}

const first = call('list', {listing: 'popular', page: 1});
assert.ok(first.entries.length > 0);
const searched = call('search', {query: 'Deity Internship', page: 1});
assert.equal(searched.entries.length, 1);
assert.ok(searched.entries[0].key.endsWith('/deity-internship-manual'));
if (!readingOnly) {
  assert.ok(first.hasNextPage);
  const second = call('list', {listing: 'popular', page: 2});
  assert.ok(second.entries.length > 0);
  assert.ok(!second.hasNextPage);
  assert.ok(!second.entries.some(i => first.entries.some(j => j.key === i.key)));
  assert.ok(call('list', {listing: 'latest', page: 1}).entries.length > 0);
  assert.equal(call('search', {query: 'no-results-unique-test-term', page: 1}).entries.length, 0);
  const filtered = call('search', {query: '', page: 1, filters: {genre: 'fantasy', status: 'ongoing', sort: 'title'}});
  assert.ok(filtered.entries.length > 0);
  // The website's cards show only two genres. Verify filtering using full detail
  // metadata, including cards whose requested genre is absent from that preview.
  for (const original of filtered.entries.slice(0, 3)) {
    const item = call('details', {item: slim(original)});
    assert.ok(item.tags.includes('Fantasy'));
    assert.equal(item.status, 'ongoing');
  }
  const filters = call('filters', {});
  assert.equal(filters.length, 4);
  assert.ok(filters.some(f => f.id === 'genre' && f.options.some(o => o.value === 'fantasy')));
  console.log(`Catalog ${first.entries.length + second.entries.length} titles, pagination, newly added, search, empty search and filters PASS`);
}

for (const original of [searched.entries[0], ...first.entries.slice(0, 2)]) {
  const item = call('details', {item: slim(original)});
  assert.equal(item.key, original.key);
  assert.equal(item.language, 'en');
  assert.ok(item.description?.length > 50);
  assert.ok(item.cover?.url);
  // Cold component invocations block Node long enough to leave idle pooled
  // sockets stale. Use independent connections for these external assertions.
  const cover = await fetch(item.cover.url, {headers: {...item.cover.headers, Connection: 'close'}, signal: AbortSignal.timeout(30000)});
  assert.equal(cover.status, 200);
  assert.match(cover.headers.get('content-type') ?? '', /^image\//);
  const chapters = call('chapters', {item: slim(item)});
  assert.ok(chapters.length > 20, 'Complete chapter list missing');
  const firstPage = call('chapters-page', {item: slim(item), page: 1});
  if (firstPage.hasNextPage) {
    const lastPage = call('chapters-page', {item: slim(item), page: firstPage.pageCount});
    assert.ok(!lastPage.hasNextPage);
    assert.equal(lastPage.entries.at(-1).key, chapters.at(-1).key);
    assert.ok(chapters.length > firstPage.entries.length, 'Older chapter pages missing');
  }
  assert.equal(new Set(chapters.map(c => c.key)).size, chapters.length);
  assert.ok(chapters[0].chapterNumber >= chapters.at(-1).chapterNumber);
  const publicChapters = chapters.filter(c => !c.isLocked && c.chapterNumber > 0);
  assert.ok(publicChapters.length > 0);
  for (const chapter of [publicChapters.at(-1), publicChapters[Math.floor(publicChapters.length / 2)], publicChapters[0]]) {
    const text = call('text', {item: slim(item), chapter});
    assert.ok(text.html?.length > 300);
    assert.ok(!/<(?:script|iframe|form|button)\b/i.test(text.html));
    assert.equal(text.baseUrl, chapter.url);
    const raw = await fetch(chapter.url, {headers: {Connection: 'close'}, signal: AbortSignal.timeout(30000)}).then(r => {assert.equal(r.status, 200); return r.text();});
    const reader = raw.slice(raw.indexOf('id="ac-r-body"'));
    const opening = [...reader.matchAll(/<p\b[^>]*>([\s\S]*?)<\/p>/g)].map(m => plain(m[1])).find(p => p.length > 40);
    assert.ok(opening, 'Official chapter prose missing');
    assert.ok(plain(text.html).includes(opening), 'Reader text differs from the official public chapter');
  }
  // A stale lock flag must not hide text which the server now makes available.
  const released = call('text', {item: slim(item), chapter: {...publicChapters[0], isLocked: true}});
  assert.ok(released.html?.length > 300);
  const locked = chapters.find(c => c.isLocked);
  if (locked) {
    let error;
    try {call('text', {item: slim(item), chapter: locked});} catch (e) {error = e;}
    assert.ok(error, 'Locked chapter unexpectedly returned text');
    assert.match(String(error.stderr), /Rainy Lotus chapter is locked/);
  }
  console.log(`${item.title}: details, cover, ${chapters.length} ordered chapters; first/middle/newest public prose and stale lock revalidation PASS; paid gate ${locked ? 'PASS' : 'NOT PRESENT'}`);
}
const login = call('auth.login', {interactive: true});
assert.equal(login.action.url, 'https://rainylotus.com/login');
assert.equal(login.action.profile, 'rainylotus');
assert.equal(call('auth.status', {}).authenticated, false);
console.log('Official login action and guest status PASS; authenticated purchases/reading NOT VERIFIED');
