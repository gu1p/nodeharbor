import { useEffect, useRef, useState } from 'react';
import { Copy, Terminal } from 'lucide-react';
import type { ActivitySnapshot } from './model';

function secondsSince(timestamp:string,now:number){return Math.max(0,Math.floor((now-Date.parse(timestamp))/1000))||0;}
function duration(seconds:number){return seconds<60?`${seconds}s`:`${Math.floor(seconds/60)}m ${seconds%60}s`;}

export function ActivityPanel({load}:{load:()=>Promise<ActivitySnapshot>}){
 const [data,setData]=useState<ActivitySnapshot|null>(null);
 const [error,setError]=useState('');const [copied,setCopied]=useState('');
 const [follow,setFollow]=useState(true);const [now,setNow]=useState(Date.now());
 const viewport=useRef<HTMLDivElement>(null);const reconnect=useRef<()=>void>(()=>{});
 useEffect(()=>{
  let disposed=false;let pending=false;
  const refresh=async()=>{
   if(pending||disposed)return;
   pending=true;
   try{const next=await load();if(!disposed){setData(next);setError('');}}
   catch(error){if(!disposed)setError(error instanceof Error?error.message:String(error));}
   finally{pending=false;}
  };
  reconnect.current=()=>{void refresh();};
  void refresh();
  const timer=setInterval(()=>{setNow(Date.now());void refresh();},1000);
  return()=>{disposed=true;clearInterval(timer);};
 },[load]);
 const last=data?.entries.at(-1);
 useEffect(()=>{if(follow&&viewport.current)viewport.current.scrollTop=viewport.current.scrollHeight;},[last?.id,follow]);
 const elapsed=data?.step?secondsSince(data.step.startedAt,now):0;
 const quiet=last?secondsSince(last.timestamp,now):elapsed;
 async function copy(){
  try{
   if(!navigator.clipboard)throw new Error('Clipboard is unavailable; select and copy the log text.');
   await navigator.clipboard.writeText((data?.entries??[]).map(entry=>`${entry.timestamp} [${entry.level.toUpperCase()}] ${entry.source}: ${entry.message}`).join('\n'));
   setCopied('Logs copied');
  }catch(error){setCopied(error instanceof Error?error.message:String(error));}
 }
 return <section className="panel activity-panel" aria-labelledby="activity-heading">
  <div className="activity-toolbar"><div className="activity-title"><Terminal size={19}/><h2 id="activity-heading">Worker activity</h2><span className={`activity-live${error?' interrupted':''}`}><span/>{error?'Disconnected':'Live'}</span></div>
   <div className="activity-controls"><label><input type="checkbox" checked={follow} onChange={event=>setFollow(event.target.checked)}/>Follow latest output</label><button disabled={!data?.entries.length} onClick={()=>void copy()}><Copy size={14}/>Copy logs</button></div>
  </div>
  {data?.step&&<div className="activity-step"><strong>{data.step.message}</strong><span>{duration(elapsed)} elapsed{data.step.timeoutSeconds?` · timeout ${duration(data.step.timeoutSeconds)}`:''}</span></div>}
  {data?.step&&quiet>=30&&<p className="activity-waiting">No new output for {duration(quiet)}. The operation is still waiting; it has not reported completion.</p>}
  {error&&<div className="alert" role="alert"><span>Live updates interrupted: {error}</span><button onClick={()=>reconnect.current()}>Reconnect logs</button></div>}
  <div ref={viewport} className="activity-log" role="log" aria-label="Worker activity log" aria-live="polite" aria-relevant="additions" aria-busy={!data&&!error} tabIndex={0} onScroll={event=>{const element=event.currentTarget;if(follow&&element.scrollHeight-element.scrollTop-element.clientHeight>32)setFollow(false);}}>
   {!data&&!error?<p className="activity-empty">Loading activity…</p>:!data?.entries.length?<p className="activity-empty">Worker activity will appear here when you prepare or start sharing.</p>:
    <ol>{data.entries.map(entry=><li key={entry.id} className={`activity-entry activity-${entry.level}`}><time dateTime={entry.timestamp}>{new Date(entry.timestamp).toLocaleTimeString([],{hour12:false})}</time><span className="activity-source">{entry.source}</span><span><b className="activity-level">{entry.level.toUpperCase()}</b>{entry.message}</span></li>)}</ol>}
  </div>
  <div className="activity-footer"><span>{data?.dropped?`${data.dropped} older entries discarded. `:''}Latest 500 entries · this app session · updates every second</span>{copied&&<span role="status">{copied}</span>}</div>
 </section>;
}
