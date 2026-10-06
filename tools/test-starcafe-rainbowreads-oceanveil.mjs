// Run signed packages through a Wasmtime invoker which provides HTTP services.
// Usage: node tools/test-starcafe-rainbowreads-oceanveil.mjs <runtime-invoker>
// Browser login/playback is deliberately a separate, authenticated app test.
import assert from 'node:assert/strict';
import {execFileSync} from 'node:child_process';
import {resolve} from 'node:path';
const runner = process.argv[2];
assert.ok(runner, 'Pass a production Wasmtime invoker with an HTTP host bridge');
const requested = new Set((process.argv[3] ?? 'starcafe,rainbowreads,oceanveil').split(','));
function call(id, kind, operation, request) {
  // Only send fields used by this operation; avoid large content in argv/errors.
  if (request.item) {
    const {key,url,title} = request.item;
    request = {...request, item:{key,url,title}};
  }
  return JSON.parse(execFileSync(resolve(runner), [resolve(`dist/packages/${kind}/en/${id}.manatan2`), `${kind}.${operation}`, JSON.stringify(request)], {encoding:'utf8', maxBuffer:16*1024*1024, timeout:120000, stdio:['ignore','pipe','pipe']}));
}
async function cover(item) {
  assert.ok(item.cover?.url, 'Cover request missing');
  const response = await fetch(item.cover.url, {headers:item.cover.headers ?? {}});
  assert.equal(response.status, 200, 'Cover request failed');
  assert.match(response.headers.get('content-type') ?? '', /^image\//);
}
for (const id of ['starcafe','rainbowreads']) {
  if (!requested.has(id)) continue;
  const first = call(id,'novel','list',{listing:'popular',page:1});
  assert.ok(first.entries.length > 0);
  const second = call(id,'novel','list',{listing:'popular',page:2});
  assert.equal(second.entries.length > 0, first.hasNextPage);
  const latest = call(id,'novel','list',{listing:'latest',page:1});
  assert.ok(latest.entries.length > 0);
  const searched = call(id,'novel','search',{query:first.entries[0].title,page:1});
  assert.ok(searched.entries.some(i => i.key === first.entries[0].key));
  const selected = id === 'starcafe' ? first.entries : first.entries.slice(0,2);
  for (const original of selected) {
    const item = call(id,'novel','details',{item:original});
    assert.equal(item.key, original.key);
    await cover(item);
    const chapters = call(id,'novel','chapters',{item});
    assert.ok(chapters.length > 0);
    assert.equal(new Set(chapters.map(c => c.key)).size, chapters.length);
    assert.ok(chapters[0].chapterNumber >= chapters.at(-1).chapterNumber);
    // Number zero can be a short supplemental fanart page, not a novel chapter.
    const publicChapters = chapters.filter(c => !c.isLocked && c.chapterNumber > 0);
    assert.ok(publicChapters.length > 0);
    for (const chapter of [publicChapters.at(-1), publicChapters[Math.floor(publicChapters.length/2)]]) {
      const text = call(id,'novel','text',{item,chapter});
      assert.ok(text.html?.length > 300, 'Reader content missing');
      assert.ok(!/<(?:script|iframe|form)\b/i.test(text.html), 'Unsafe reader markup');
    }
    console.log(`${id}: details, cover, ${chapters.length} ordered chapters, first/middle public text PASS`);
  }
  console.log(`${id}: live catalog, pagination, latest and search PASS`);
}
if (requested.has('oceanveil')) {
const videos = call('oceanveil','video','list',{listing:'popular',page:1});
assert.equal(videos.entries.length,30);
assert.ok(videos.hasNextPage);
const second = call('oceanveil','video','list',{listing:'popular',page:2});
assert.equal(second.entries.length,30);
assert.ok(!second.entries.some(i => videos.entries.some(j => j.key === i.key)));
assert.ok(call('oceanveil','video','list',{listing:'latest',page:1}).entries.length > 0);
const searched = call('oceanveil','video','search',{query:videos.entries[0].title,page:1});
assert.ok(searched.entries.some(i => i.key === videos.entries[0].key));
const detail = call('oceanveil','video','details',{item:videos.entries[0]});
assert.equal(detail.key,videos.entries[0].key);
await cover(detail);
const episodes = call('oceanveil','video','episodes',{item:detail});
assert.ok(episodes.length > 0);
assert.ok(episodes[0].episodeNumber >= episodes.at(-1).episodeNumber);
console.log(`oceanveil: live catalog, pagination, latest, search, details, cover and ${episodes.length} episodes PASS`);
console.log('OceanVeil full playback: NOT VERIFIED (requires authorized account/browser; DRM unsupported).');
}
