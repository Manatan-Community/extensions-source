# Porting Playbook

Use this when adapting an existing site source into a Manatan-native extension.

1. Identify the media kind: manga, video, or novel.
2. Create one package for that media kind only.
3. Preserve the public source ID, language, name, base URL, and content rating when they are stable.
4. Add small fixtures before writing parsing logic.
5. Implement list/search/details first.
6. Add the media-specific content path:
   - manga: chapters and pages
   - video: episodes, hosters, and streams
   - novel: chapters and text
7. Move repeated parsing, URL, header, or playlist logic into `shared/`.
8. Build and validate the `.manatan` package.

Do not silently skip site behavior. If Manatan needs a new SDK or host capability, document that clearly in the pull request.
