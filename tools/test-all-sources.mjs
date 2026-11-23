// Signed-package live audit against a dedicated Manatan production backend.
// node tools/test-all-sources.mjs http://127.0.0.1:PORT http://127.0.0.1:FEED report.json [package-id]
// No guest, HTTP, database, catalog, or reader implementation is replaced.
// Use the native host fast-release profile for mobile validation: unoptimized
// Rust can exhaust per-call budgets and produce misleading timeout failures.
import assert from 'node:assert/strict';
import {readFile, writeFile, mkdir, rename} from 'node:fs/promises';
import {dirname, join} from 'node:path';
import {spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';
const [base, feed, output, only] = process.argv.slice(2);
for (const url of [base, feed]) assert.match(url ?? '', /^http:\/\/127\.0\.0\.1:\d+$/);
assert.ok(output);
await mkdir(dirname(output), {recursive: true});
const sampleQueries = JSON.parse(process.env.AUDIT_SAMPLE_QUERIES ?? '{}');
const sampleChapters = JSON.parse(process.env.AUDIT_SAMPLE_CHAPTERS ?? '{}');
const deviceVideoUI = process.env.AUDIT_DEVICE_VIDEO_UI === '1';
const localPortMap = JSON.parse(process.env.AUDIT_LOCAL_PORT_MAP ?? '{}');
function transportURL(path) {
  const url = new URL(path, base);
  if (['127.0.0.1', 'localhost'].includes(url.hostname) && localPortMap[url.port]) {
    url.hostname = '127.0.0.1'; url.port = String(localPortMap[url.port]);
  }
  return url;
}
const rows = JSON.parse(await readFile(new URL('../dist/catalog.min.json', import.meta.url)));
const report = process.env.AUDIT_RESUME ? JSON.parse(await readFile(output)) : {startedAt: new Date().toISOString(), sdk: 'd13370a1abc220eda0a805ca9eef0a22d7cadf10', packages: []};
report.buildProfile = process.env.AUDIT_BUILD_PROFILE ?? report.buildProfile ?? 'not recorded';
report.platform = process.env.AUDIT_PLATFORM ?? report.platform ?? 'not recorded';
report.videoPlaybackRequiresSeparateDeviceProof = deviceVideoUI;
let pendingWrite = Promise.resolve();
const persist = () => pendingWrite = pendingWrite.then(async () => {
  await writeFile(output+'.tmp', JSON.stringify(report, null, 2) + '\n');
  await rename(output+'.tmp',output);
});
function check(value, message) { if (!value) throw Error(message); }
async function request(path, body, limit = 120_000) {
  const response = await fetch(new URL(path, base), {method: body ? 'POST' : 'GET', headers: body ? {'Content-Type': 'application/json'} : {}, body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(limit)});
  const text = await response.text();
  check(response.ok, `HTTP ${response.status}: ${text.slice(0, 1800)}`);
  return JSON.parse(text);
}
const entries = value => Array.isArray(value) ? value : value.entries ?? value.animeList ?? value.mangaList ?? value.animes ?? value.mangas ?? [];
async function stage(row, name, fn) {
  const started = Date.now();
  try { const result = await fn(); row.stages[name] = {ok: true, ms: Date.now()-started}; await persist(); return result; }
  catch (error) {row.stages[name] = {ok: false, ms: Date.now()-started, error: String(error.message ?? error)}; await persist(); return undefined;}
}
async function image(url, label) {
  check(url, `${label}: missing image URL`);
  const target = transportURL(url);
  if (process.env.AUDIT_REQUIRE_LOCAL_IMAGES === '1') {
    check(['127.0.0.1', 'localhost'].includes(target.hostname), `${label}: requires device-side image route; host CDN fetching is not a device test`);
  }
  const response = await fetch(target, {signal: AbortSignal.timeout(90_000)});
  if (!response.ok) throw Error(`${label}: HTTP ${response.status}: ${(await response.text()).slice(0, 800)}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  check(bytes.length > 100, `${label}: empty image`);
  // Decode actual proxy bytes; a 200 HTML challenge is not a working cover.
  const path = join(dirname(output), 'image-sample-' + label.replace(/[^a-z0-9_-]/gi, '_'));
  await writeFile(path, bytes);
  await new Promise((resolve, reject) => {
    const process = spawn(globalThis.process.env.FFMPEG_BIN ?? 'ffmpeg', ['-nostdin', '-v', 'error', '-i', path, '-frames:v', '1', '-f', 'null', '-']);
    let stderr = ''; process.stderr.on('data', d => stderr += d); process.on('error', reject);
    process.on('exit', code => code === 0 ? resolve() : reject(Error(`${label}: image decode failed: ${stderr.slice(-700)}`)));
  });
  return bytes.length;
}
async function decodeVideo(path, headers = []) {
  if (process.env.MANATAN_MPV_LIBRARY) return new Promise((resolve,reject)=>{
    const child=spawn(process.env.PYTHON_BIN??'python3',[fileURLToPath(new URL('./test-native-playback.py',import.meta.url))]);
    let stdout='',stderr='';const timer=setTimeout(()=>child.kill('SIGKILL'),90_000);
    child.stdout.on('data',d=>stdout+=d);child.stderr.on('data',d=>stderr=(stderr+d).slice(-1000));
    child.on('error',reject);child.on('exit',code=>{clearTimeout(timer);let result;try{result=JSON.parse(stdout);}catch{};code===0&&result?.ok?resolve(result):reject(Error(result?.error??`Native playback failed (${code}): ${stderr}`));});
    child.stdin.end(JSON.stringify({url:new URL(path,base).href,headers}));
  });
  await new Promise((resolve, reject) => {
    const headerText=(Array.isArray(headers)?headers:Object.entries(headers).map(([name,value])=>({name,value}))).map(h=>`${h.name}: ${h.value}\r\n`).join('');
    const process = spawn(globalThis.process.env.FFMPEG_BIN ?? 'ffmpeg', ['-nostdin', '-v', 'error', '-rw_timeout', '30000000', ...(headerText?['-headers',headerText]:[]), '-i', new URL(path, base).href, '-t', '3', '-frames:v', '36', '-progress', 'pipe:1', '-f', 'null', '-']);
    let frames=0;process.stdout.on('data',d=>{for(const match of d.toString().matchAll(/^frame=(\d+)/gm))frames=Math.max(frames,Number(match[1]));});
    let stderr = '', settled = false;
    const timer = setTimeout(() => {process.kill('SIGKILL');}, 90_000);
    process.stderr.on('data', d => stderr = (stderr+d).slice(-1800));
    process.on('error', e => {clearTimeout(timer); reject(e);});
    process.on('exit', code => {clearTimeout(timer); if (!settled) {settled=true; code === 0 && frames>0 ? resolve() : reject(Error(`Video decode failed (${code}, ${frames} frames): ${stderr}`));}});
  });
}
async function auditPackage(pkg) {
  const media = pkg.contentType, prefix = media === 'manga' ? '' : media === 'video' ? '/anime' : '/novel';
  const row = report.packages.find(row=>row.id===pkg.id);
  if (!row.stages.install?.ok) return;
  row.variants=[];
  const all = await stage(row, 'source-registration', () => request(`/api/v1${prefix}/source/list`));
  if (!all) return;
  const registered = entries(all).filter(source => source.pkgName === pkg.pkgName || source.extension?.pkgName === pkg.pkgName || source.extensionId === pkg.pkgName);
  row.registeredCount = registered.length;
  // A missing source is a registration failure, never an upstream outage.
  if (!registered.length) { row.stages['source-registration'] = {ok:false,error:'No registered source matched package',sample:entries(all).slice(-2)}; await persist(); return; }
  const primary = registered.find(s => s.lang === 'en') ?? registered.find(s => ['all','multi'].includes(s.lang)) ?? registered[0];
  let selected;
  let nextVariant=0;
  const variants=async()=> { for (;;) {
    const source=registered[nextVariant++]; if(!source) return;
    const variant = {id: String(source.id), lang: source.lang, name: source.name, stages:{}}; row.variants.push(variant);
    const root = `/api/v1${prefix}/source/${source.id}`;
    const popular = await stage(variant, 'catalog', () => request(`${root}/popular?page=1`));
    variant.catalogCount = popular ? entries(popular).length : 0;
    if (source.id === primary.id) selected = {source, root, popular};
  }};
  await Promise.all(Array.from({length:Math.min(3,registered.length)},variants));
  if (!selected?.popular) return;
  const {source, root, popular} = selected;
  const items = entries(popular); row.catalogCount = items.length;
  if (!items.length) {row.stages.catalog={ok:false,error:'Upstream returned an empty catalog'}; await persist();return;}
  if (popular.hasNextPage) await stage(row, 'catalog-page2', async () => {
    const next = entries(await request(`${root}/popular?page=2`));
    check(next.length && next[0].url !== items[0].url, 'Empty or repeated second catalog page');
  });
  let first = items.find(item => !item.title?.match(/\b(?:18\+|adult)\b/i)) ?? items[0];
  if (sampleQueries[pkg.id]) {
    const found = await stage(row, 'sample-selection', () => request(`${root}/search?query=${encodeURIComponent(sampleQueries[pkg.id])}&page=1`));
    if (entries(found ?? []).length) first = entries(found)[0];
  }
  row.sample = {title:first.title, key:first.key ?? first.id};
  const manifestSource = pkg.sources.find(s => s.lang === source.lang) ?? pkg.sources[0];
  if (manifestSource?.capabilities?.latest) await stage(row, 'latest', async () => {check(entries(await request(`${root}/latest?page=1`)).length, 'Empty latest catalog');});
  if (manifestSource?.capabilities?.search) await stage(row, 'search', async () => {
    const query = first.title.trim();
    row.searchQuery = query;
    const found = entries(await request(`${root}/search?query=${encodeURIComponent(query)}&page=1`));
    check(found.length, `No search results for ${JSON.stringify(query)}`);
    check(found.some(i => (i.key ?? i.url) === (first.key ?? first.url) || i.title === first.title), 'Search did not return sampled title');
  });
  if (media === 'novel') {
    const details = await stage(row, 'details', async () => {const value=await request(`${root}/details`,{item:first});check(value.title,'Missing details title');return value;});
    const item = details ?? first;
    if (item.cover) row.coverBytes = await stage(row, 'cover', () => image(item.cover, pkg.id));
    else { row.stages.cover={ok:true,skipped:'Source did not provide a cover; reader uses a placeholder'}; await persist(); }
    if (manifestSource?.capabilities?.operations?.includes('novel.materialize-book')) {
      const book = await stage(row, 'materialize-book', () => request('/api/v1/novel/library', {sourceId:String(source.id),item,inLibrary:false},180_000));
      if (book?.id) row.textCharacters = await stage(row, 'reader', async () => {
        const text=await request(`/api/v1/novel/library/${encodeURIComponent(book.id)}/chapters/0`);
        check(text.html?.length>150,'Materialized chapter has no readable HTML'); return text.html.length;
      });
      return;
    }
    const chapters = await stage(row, 'chapters', async () => {const value=await request(`${root}/chapters/page?page=1`,{item});check(entries(value).length,'Empty chapter list');return value;});
    if (!chapters) return;
    row.chapterCount=entries(chapters).length;
    let readable = entries(chapters).filter(c => !c.isLocked);
    if (chapters.hasNextPage && chapters.pageCount > 1) {
      const older = await stage(row, 'chapter-pagination', () => request(`${root}/chapters/page?page=${chapters.pageCount}`,{item}));
      if (older) readable = entries(older).filter(c => !c.isLocked);
    }
    const chapter = readable.at(-1);
    if (!chapter) {row.stages.reader={ok:false,error:'No freely readable chapter in sampled book'};await persist();return;}
    row.textCharacters = await stage(row, 'reader', async () => {
      const text=await request(`${root}/text`,{item,chapter});
      check(!/<script\b|<iframe\b/i.test(text.html??''),'Unsafe chapter HTML');
      const characters=text.html?.length || text.text?.length || (text.blocks??[]).filter(b=>b.type==='text').reduce((n,b)=>n+(b.text?.length??0),0);
      // Typed image chapters are valid reader content. Fetch and decode the
      // normal opaque Manatan image route rather than accepting a block alone.
      const imageBlock=(text.blocks??[]).find(b=>b.type==='image'&&b.image?.url);
      if(imageBlock) row.chapterImageBytes=await image(imageBlock.image.url,pkg.id+'-chapter');
      check(characters>150 || row.chapterImageBytes>100,'Missing readable chapter text or decoded image');
      return characters;
    });
    return;
  }
  row.itemId = first.id;
  const itemRoot = `/api/v1${media === 'video' ? '/anime' : '/manga'}/${first.id}`;
  const details = await stage(row, 'details', async () => {const value=await request(itemRoot+'?onlineFetch=true');check(value.initialized,'Metadata refresh failed or returned cached uninitialized data');return value;});
  const artwork = details?.thumbnailUrl ?? first.thumbnailUrl;
  const localArtwork = artwork && (artwork.startsWith('/api/') || /^https?:\/\/(?:127\.0\.0\.1|localhost)(?::|\/)/.test(artwork));
  row.coverBytes = await stage(row, 'cover', () => image(localArtwork ? artwork : `${itemRoot}/thumbnail`,pkg.id));
  const chapters = await stage(row, media === 'video' ? 'episodes' : 'chapters', async () => {
    const value = entries(await request(itemRoot+(media === 'video' ? '/episodes' : '/chapters')+'?onlineFetch=true',undefined,180_000));check(value.length,'No episodes/chapters');return value;
  });
  if (!chapters) return;
  row.chapterCount=chapters.length;
  let chapter = chapters.filter(c => !c.isLocked).at(-1) ?? chapters.at(-1);
  if (sampleChapters[pkg.id] !== undefined) {
    chapter = await stage(row, 'chapter-selection', async () => {
      const selected = chapters.find(c => Number(c.chapterNumber ?? c.episodeNumber) === Number(sampleChapters[pkg.id]));
      check(selected, 'Requested sample chapter/episode was not returned by the source');
      return selected;
    });
    if (!chapter) return;
  }
  row.chapterSample = {index: chapter.index, number: chapter.chapterNumber ?? chapter.episodeNumber};
  const chapterRoot = `${itemRoot}/${media === 'video' ? 'episode' : 'chapter'}/${chapter.index}`;
  if (media === 'manga') {
    const pages = await stage(row, 'pages', () => request(`${chapterRoot}/pages`));
    if (pages) row.pageBytes = await stage(row, 'reader', () => image(`${chapterRoot}/page/0`, pkg.id+'-page'));
  } else {
    const streams = await stage(row, 'streams', async () => {const value=await request(`${chapterRoot}/videos?onlineFetch=true`,undefined,180_000);check(Array.isArray(value)&&value.length,'No playable streams');return value;});
    if (!streams?.length) return;
    row.streams=[];
    if (deviceVideoUI) {
      row.devicePlayback = {verified: false, reason: 'Requires actual device player/UI validation; no host decoder substituted'};
      row.videoOptions = streams.map((video,index)=>({index,name:video.videoTitle??video.name,quality:video.quality}));
      await persist(); return;
    }
    for (let index=0;index<Math.min(streams.length,4);index++) {
      const result={index,stages:{}};row.streams.push(result);
      // Match Flutter's nativePlayerVideoSourceUrl: direct DASH stays direct;
      // sources requiring the host proxy must retain its transforms/headers.
      const video=streams[index];
      const playback=video.requiresProxy&&video.proxyUrl ? video.proxyUrl : video.url || video.videoUrl || video.ffmpegSourceUrl;
      result.decoded=await stage(result,'decode',()=>decodeVideo(playback,video.headers));
      if(result.stages.decode.ok) break;
    }
  }
}
// Serialize package installs (the registry is shared), then audit independent sources.
const selected=rows.filter(pkg=>!only||only.split(',').includes(pkg.id));
if (process.env.AUDIT_REVERSE_ORDER === '1') selected.reverse();
const alreadyInstalled=process.env.AUDIT_SKIP_INSTALL ? await request('/api/v1/manatan/extension/list') : [];
for (const pkg of selected) {
 const prefix=pkg.contentType==='manga'?'':pkg.contentType==='video'?'/anime':'/novel';
 const existing=report.packages.find(row=>row.id===pkg.id);
 if(existing?.stages.install?.ok) continue;
 const row=existing??{id:pkg.id,name:pkg.name,media:pkg.contentType,version:pkg.version,stages:{},variants:[]};if(!existing)report.packages.push(row);
 if(alreadyInstalled.some(item=>item.pkgName===pkg.pkgName&&item.installed&&item.versionCode===pkg.versionCode)) {
  row.stages.install={ok:true,evidence:'Confirmed signed package and version in production installed registry'};await persist();continue;
 }
 await stage(row,'install',()=>request(`/api/v1${prefix}/extension/install/${encodeURIComponent(pkg.pkgName)}?repo=${encodeURIComponent(feed+'/'+pkg.contentType+'.min.json')}`,pkg.contentType==='novel'?{}:undefined));
 console.log(JSON.stringify({id:row.id,install:row.stages.install}));
}
let nextPackage=0;
const worker=async()=> {for (;;) {
 const pkg=selected[nextPackage++];if(!pkg)return;
 try {await auditPackage(pkg);} catch(error) {
  const row=report.packages.find(row=>row.id===pkg.id);row.stages.audit={ok:false,error:String(error.stack??error)};
 }
 await persist();
 const row=report.packages.find(row=>row.id===pkg.id);
 console.log(JSON.stringify({id:row.id,registered:row.registeredCount,stages:row.stages,variantFailures:row.variants.filter(v=>!v.stages.catalog?.ok).length}));
}};
await Promise.all(Array.from({length:Math.max(1,Math.min(3,Number(process.env.AUDIT_WORKERS)||1))},worker));
report.finishedAt=new Date().toISOString();await persist();
process.exitCode=report.packages.some(row=>Object.values(row.stages).some(stage=>!stage.ok)||row.variants.some(variant=>!variant.stages.catalog?.ok)||(row.streams?.length&&!row.streams.some(stream=>stream.stages.decode?.ok)))?1:0;
