import {mkdir, writeFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {resolve, join} from 'node:path';
import {spawnSync} from 'node:child_process';
import {randomBytes} from 'node:crypto';

const dir=resolve(process.argv[2]||'tmp/fixture-media');
await mkdir(join(dir,'hls'),{recursive:true});
await mkdir(join(dir,'encrypted'),{recursive:true});
const run=args=>{const r=spawnSync('ffmpeg',['-hide_banner','-loglevel','error','-y',...args],{stdio:'inherit'});if(r.status!==0)process.exit(r.status||1);};
const font='/System/Library/Fonts/Supplemental/Arial.ttf';
const stamp=`drawtext=${existsSync(font)?`fontfile=${font}:`:''}text='TV RUST  %{pts\\:hms}':x=24:y=24:fontsize=28:fontcolor=white:box=1:boxcolor=black@0.8`;
const hasDrawtext=spawnSync('ffmpeg',['-hide_banner','-filters'],{encoding:'utf8'}).stdout.includes(' drawtext ');
run(['-f','lavfi','-i','testsrc=size=640x360:rate=24','-f','lavfi','-i','sine=frequency=440:sample_rate=48000','-t','120',...(hasDrawtext?['-vf',stamp]:[]),
  '-c:v','libx264','-preset','ultrafast','-crf','27','-g','48','-profile:v','baseline','-pix_fmt','yuv420p','-c:a','aac','-ac','2','-b:a','96k','-movflags','+faststart',join(dir,'demo.mp4')]);
run(['-i',join(dir,'demo.mp4'),'-c','copy','-f','hls','-hls_time','4','-hls_playlist_type','vod','-hls_segment_filename',join(dir,'hls','segment-%03d.ts'),join(dir,'hls','media.m3u8')]);
await writeFile(join(dir,'hls','master.m3u8'),'#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1800000,RESOLUTION=640x360,CODECS="avc1.42c01e,mp4a.40.2"\nmedia.m3u8\n');
await writeFile(join(dir,'encrypted','key.bin'),randomBytes(16));
await writeFile(join(dir,'encrypted','key-info'),`key.bin\n${join(dir,'encrypted','key.bin')}\n`);
run(['-i',join(dir,'demo.mp4'),'-c','copy','-f','hls','-hls_time','4','-hls_playlist_type','vod','-hls_key_info_file',join(dir,'encrypted','key-info'),'-hls_segment_filename',join(dir,'encrypted','segment-%03d.ts'),join(dir,'encrypted','media.m3u8')]);
run(['-ss','2','-i',join(dir,'demo.mp4'),'-frames:v','1','-vf','scale=360:540',join(dir,'poster.png')]);
console.log(`受控媒体已生成：${dir}`);
