import http from 'node:http';
import {readFile,stat,mkdir,appendFile} from 'node:fs/promises';
import {createReadStream} from 'node:fs';
import {resolve,join,extname} from 'node:path';

const root=resolve(process.env.FIXTURE_DIR||'tmp/fixture-media');
const port=Number(process.env.FIXTURE_PORT||55321);
const base=process.env.FIXTURE_PUBLIC_URL||`http://127.0.0.1:${port}`;
const audit=resolve(process.env.FIXTURE_AUDIT||'tmp/fixture-events.jsonl');
await mkdir(resolve(audit,'..'),{recursive:true});
const events=[];
let configState='valid';
const movies=()=>[
 {vod_id:1001,vod_name:'TV Rust 验收电影',type_name:'电影',vod_year:'2026',vod_play_from:'直链MP4',vod_play_url:`正片$${base}/media/demo.mp4`},
 {vod_id:1002,vod_name:'TV Rust 验收剧集',type_name:'电视剧',vod_year:'2026',vod_play_from:'HLS原画',vod_play_url:`第01集$${base}/media/hls/master.m3u8#第02集$${base}/media/hls/media.m3u8#`},
 {vod_id:1003,vod_name:'TV Rust 跨域回退',type_name:'电影',vod_year:'2026',vod_play_from:'HLS跨域',vod_play_url:`正片$${base}/nocors/hls/master.m3u8`},
 {vod_id:1004,vod_name:'TV Rust 加密清单',type_name:'电影',vod_year:'2026',vod_play_from:'HLS加密',vod_play_url:`正片$${base}/media/encrypted/media.m3u8`},
 {vod_id:1005,vod_name:'TV Rust 网页地址',type_name:'电影',vod_year:'2026',vod_play_from:'网页入口',vod_play_url:`正片$${base}/player-page`},
].map(v=>({...v,vod_pic:`${base}/media/poster.png`,vod_content:'本项目生成的测试图、持续变化的时间码与 440Hz 音频，用于验证播放、拖动和续播。',vod_actor:'受控测试素材',vod_director:'TV Rust',vod_remarks:v.vod_id===1002?'全2集':'120秒'}));
function send(res,data,status=200){res.writeHead(status,{'content-type':'application/json;charset=utf-8','cache-control':'no-store'});res.end(JSON.stringify(data));}
const server=http.createServer(async(req,res)=>{
 try{
  const url=new URL(req.url,base);
  if(url.pathname==='/__stats'){send(res,{events});return;}
  if(url.pathname==='/__reset'&&req.method==='POST'){events.length=0;send(res,{ok:true});return;}
  if(url.pathname==='/__config'&&req.method==='POST'){
   let body='';for await(const part of req){body+=part;if(body.length>8192)throw new Error('too large');}
   const state=JSON.parse(body).state;if(!['valid','invalid','removed'].includes(state)){send(res,{error:'invalid state'},400);return;}
   configState=state;send(res,{ok:true});return;
  }
  const event={at:new Date().toISOString(),path:url.pathname,method:req.method,range:req.headers.range||'',userAgent:req.headers['user-agent']||'',hasAuthorization:!!req.headers.authorization,hasCookie:!!req.headers.cookie};
  events.push(event);if(events.length>2000)events.shift();await appendFile(audit,JSON.stringify(event)+'\n');
  if(url.pathname==='/config.json'){
   if(configState==='invalid'){res.writeHead(200,{'content-type':'text/plain'});res.end('invalid subscription');return;}
   send(res,{cache_time:7200,api_site:configState==='removed'?{}:{'fixture.example':{name:'本机验收源',api:`${base}/api`,detail:base}}});return;
  }
  if(url.pathname==='/failed-api'){send(res,{error:'controlled failure'},503);return;}
  if(url.pathname==='/player-page'){res.writeHead(200,{'content-type':'text/html'});res.end('<!doctype html><html><body>需要额外解析的受控播放器网页</body></html>');return;}
  if(url.pathname==='/api'||url.pathname==='/alternate-api'){
   let list=movies();const ids=url.searchParams.get('ids'),q=url.searchParams.get('wd');
   if(url.pathname==='/alternate-api')list=list.map(v=>({...v,vod_play_from:v.vod_id===1002?'换源HLS':v.vod_play_from,vod_play_url:v.vod_play_url.replace('第01集$','第一集$').replace('第02集$','第二集$')}));
   if(ids)list=list.filter(v=>String(v.vod_id)===ids);
   if(q)list=list.filter(v=>v.vod_name.toLowerCase().includes(q.toLowerCase()));
   // 用字符串页码以及 text/html Content-Type 验证宽容的 JSON 接口解析。
   res.writeHead(200,{'content-type':'text/html;charset=utf-8','access-control-allow-origin':'*'});
   res.end(JSON.stringify({code:1,msg:'数据列表',page:'1',pagecount:1,limit:'20',total:list.length,list,class:[{type_id:1,type_name:'电影'},{type_id:2,type_name:'电视剧'}]}));return;
  }
  const nocors=url.pathname.startsWith('/nocors/');
  const relative=decodeURIComponent(url.pathname.replace(/^\/(media|nocors)\//,''));
  const file=resolve(root,relative);if(!file.startsWith(root+'/')){send(res,{error:'not found'},404);return;}
  const st=await stat(file);if(!st.isFile()){send(res,{error:'not found'},404);return;}
  const mime={'.mp4':'video/mp4','.m3u8':'application/vnd.apple.mpegurl','.ts':'video/mp2t','.png':'image/png','.bin':'application/octet-stream'}[extname(file)]||'application/octet-stream';
  let start=0,end=st.size-1,status=200;
  if(req.headers.range&&mime!=='application/vnd.apple.mpegurl'){
   const m=/^bytes=(\d*)-(\d*)$/.exec(req.headers.range);
   if(!m||(!m[1]&&!m[2])){res.writeHead(416,{'content-range':`bytes */${st.size}`});res.end();return;}
   if(m[1]){start=Number(m[1]);if(m[2])end=Math.min(Number(m[2]),end);}else{start=Math.max(0,st.size-Number(m[2]));}
   if(start>=st.size||end<start){res.writeHead(416,{'content-range':`bytes */${st.size}`});res.end();return;}status=206;
  }
  const headers={'content-type':mime,'content-length':end-start+1,'accept-ranges':'bytes','cache-control':'no-store'};
  if(!nocors){headers['access-control-allow-origin']='*';headers['access-control-expose-headers']='Content-Range,Accept-Ranges';}
  if(status===206)headers['content-range']=`bytes ${start}-${end}/${st.size}`;
  res.writeHead(status,headers);if(req.method==='HEAD'){res.end();return;}
  createReadStream(file,{start,end}).pipe(res);
 }catch{if(!res.headersSent)send(res,{error:'not found'},404);else res.destroy();}
});
server.listen(port,process.env.FIXTURE_BIND||'127.0.0.1',()=>console.log(`受控源：${base}/config.json`));
