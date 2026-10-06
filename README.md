spotifast (forked) with additions (credits to Carmine Paolino)

- can download songs as different audio formats (individually, songs, albums etc)
- can add songs from third party sources like youtube or soundcloud into your playlist
- can add songs from your pc into your playlist
- has romanization for japanese, chinese and korean songs
- caches the playlists because initial connection with spotify is very slow
- adds musixmatch, lrclib, netease and genius as extra fallback lyrics providers


all of this is still fully in rust so there is no performance drop, still 0% cpu usage on idle and around 100mb of ram
dependencies for these are deno, yt-dlp and ffmpeg, downloaded in tools/ only if you don't already have any of these on your pc
