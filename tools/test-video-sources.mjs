// Bounded live audit of the eight native video packages in a dedicated test app.
// node tools/test-video-sources.mjs http://127.0.0.1:<task-port> [source-name]
// This checks transport bytes, not decoded playback. Follow with 30s + seek UI tests.
// Output deliberately excludes stream URLs, request headers and response bodies.
import assert from 'node:assert/strict';

const base = process.argv[2];
assert.match(base ?? '', /^http:\/\/127\.0\.0\.1:\d+$/);
const cases = [
  ['Anikoto', 'Solo Leveling', 'Solo Leveling'],
  ['AniWave (Unoriginal)', 'One Piece', 'One Piece'],
  ['AsianCTV', 'My Bias, My Boss', 'My Bias, My Boss (2026)'],
  ['ShuttleTV', 'Spirited Away', 'Spirited Away'],
  ['ReAnime', 'One Piece', 'ONE PIECE'],
  ['AnimeOnsen', 'Studio Apartment', 'Studio Apartment, Good Lighting, Angel Included'],
  ['AniZone', 'One Piece', 'One Piece'],
  ['WIT ANIME', 'One Piece', 'One Piece'],
];
let stage;
class AuditFailure extends Error {
  constructor(code, status) { super(code); this.code = code; this.status = status; }
}
async function bounded(url, limit = 2 * 1024 * 1024, headers = {}) {
  // An explicit full refresh may traverse 1,000+ episodes on a paged source.
  // Keep it bounded, but distinguish that audit from normal playback latency.
  const response = await fetch(url, {headers, signal: AbortSignal.timeout(180_000)});
  const reader = response.body?.getReader();
  const chunks = [];
  let length = 0;
  if (reader) {
    try {
      while (length < limit) {
        const {value, done} = await reader.read();
        if (done) break;
        const part = value.subarray(0, limit - length);
        chunks.push(part); length += part.length;
      }
    } finally { await reader.cancel(); }
  }
  return {status: response.status, type: response.headers.get('content-type') ?? '', bytes: Buffer.concat(chunks)};
}
async function json(path) {
  const result = await bounded(base + path);
  if (result.status !== 200) throw new AuditFailure('http_error', result.status);
  try { return JSON.parse(result.bytes.toString()); }
  catch { throw new AuditFailure('invalid_json'); }
}
function check(value, code) { if (!value) throw new AuditFailure(code); }
function local(candidate, parent = base) {
  const url = new URL(candidate, parent);
  check(url.origin === base, 'unexpected_non_proxy_resource');
  return url.href;
}
async function media(candidate) {
  const result = await bounded(local(candidate), 65536, {Range: 'bytes=0-65535'});
  check(result.status === 200 || result.status === 206, `media_http_${result.status}`);
  check(result.bytes.length > 256, 'empty_media');
  const looksTs = result.bytes[0] === 0x47 && result.bytes[188] === 0x47;
  const box = result.bytes.subarray(4, 8).toString();
  const looksMp4 = ['ftyp', 'styp', 'moof', 'sidx'].includes(box);
  // Some CDNs label valid binary segments as text/html or JSON. Reject an
  // actual error body, not just a misleading Content-Type header.
  check(looksTs || looksMp4 || !/^text\/html|application\/json/.test(result.type),
    'non_media_response');
  return {status: result.status, type: result.type, sampledBytes: result.bytes.length};
}
async function hls(url, depth = 0) {
  check(depth < 4, 'playlist_recursion');
  const result = await bounded(local(url));
  check(result.status === 200, `playlist_http_${result.status}`);
  const text = result.bytes.toString();
  check(text.startsWith('#EXTM3U'), 'not_hls');
  const lines = text.split(/\r?\n/).filter(line => line && !line.startsWith('#'));
  check(lines.length > 0, 'empty_playlist');
  if (text.includes('#EXT-X-STREAM-INF:')) return hls(local(lines[0], url), depth + 1);
  const map = text.match(/#EXT-X-MAP:.*URI="([^"]+)"/);
  const resources = [];
  if (map) resources.push(await media(local(map[1], url)));
  for (const index of new Set([0, Math.floor(lines.length / 2), lines.length - 1])) {
    resources.push(await media(local(lines[index], url)));
  }
  return {segments: lines.length, samples: resources};
}

const sources = await json('/api/v1/anime/source/list');
let failures = 0;
for (const [name, query, title] of cases.filter(([name]) => !process.argv[3] || name === process.argv[3])) {
  const report = {name};
  try {
    stage = 'installed-source';
    const source = sources.find(source => source.name === name);
    check(source, 'missing_source');
    const root = `/api/v1/anime/source/${source.id}`;
    stage = 'catalog';
    const popular = await json(`${root}/popular?page=1`);
    const items = popular.animeList ?? popular.animes ?? [];
    check(items.length, 'empty_catalog');
    report.catalogCount = items.length;
    if (popular.hasNextPage) {
      stage = 'catalog-page2';
      const page2 = await json(`${root}/popular?page=2`);
      const next = page2.animeList ?? page2.animes ?? [];
      check(next.length, 'empty_second_page');
      check(next[0].url !== items[0].url, 'repeated_page');
    }
    stage = 'search';
    const search = await json(`${root}/search?query=${encodeURIComponent(query)}&page=1`);
    const item = (search.animeList ?? search.animes ?? []).find(item => item.title.toLowerCase() === title.toLowerCase());
    check(item, 'missing_search_result');
    const itemRoot = `/api/v1/anime/${item.id}`;
    stage = 'details';
    const details = await json(`${itemRoot}?onlineFetch=true`);
    check(details.initialized && details.description?.length > 40, 'incomplete_details');
    stage = 'episodes';
    const episodes = await json(`${itemRoot}/episodes?onlineFetch=true`);
    check(episodes.length, 'empty_episodes');
    report.episodeCount = episodes.length;
    // Database array order is not playback order.
    const first = episodes.filter(e => e.episodeNumber >= 1).reduce((a, b) => !a || b.episodeNumber < a.episodeNumber ? b : a, null);
    check(first, 'missing_first_episode');
    const episodeRoot = `${itemRoot}/episode/${first.index}`;
    stage = 'streams';
    const videos = await json(`${episodeRoot}/videos?onlineFetch=true`);
    check(videos.length, 'empty_streams');
    report.streams = [];
    for (const [index, video] of videos.entries()) {
      stage = 'media';
      const stream = {index, format: video.format, subtitles: video.subtitleTracks?.length ?? 0, audioTracks: video.audioTracks?.length ?? 0};
      try {
        // Use the same proxy entry point as native playback: /playlist does
        // not execute a package's guest playlist transform (e.g. ReAnime).
        if (video.isHls) stream.transport = await hls(base + `${episodeRoot}/video/${index}`);
        else if (video.isDash) {
          const manifest = await bounded(base + `${episodeRoot}/video/${index}`, 512 * 1024);
          check([200, 206].includes(manifest.status) && /<MPD\b/.test(manifest.bytes.toString()), 'invalid_dash_manifest');
          stream.transport = {status: manifest.status, type: manifest.type};
        } else stream.transport = await media(`${episodeRoot}/video/${index}`);
        if (video.subtitleTracks?.length) {
          stage = 'subtitles';
          const subtitle = await bounded(local(video.subtitleTracks[0].url));
          check(subtitle.status === 200 && subtitle.bytes.length > 100, 'subtitle_failed');
          stream.subtitleStatus = subtitle.status;
        }
        stream.ok = true;
      } catch (error) { stream.ok = false; stream.error = error instanceof AuditFailure ? error.code : error.name; }
      report.streams.push(stream);
    }
    check(report.streams.some(stream => stream.ok), 'no_working_stream');
    report.ok = true;
  } catch (error) {
    failures++;
    report.ok = false; report.stage = stage;
    report.error = error instanceof AuditFailure ? error.code : error.name;
    if (error.status) report.status = error.status;
  }
  console.log(JSON.stringify(report));
}
process.exitCode = failures ? 1 : 0;
