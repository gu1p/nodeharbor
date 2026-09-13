import {useCallback,useEffect,useState} from 'react';
import type {Backend,UpdateAction,UpdateStatus} from './model';

export function UpdatesPanel({backend}:{backend:Backend}) {
 const [status,setStatus]=useState<UpdateStatus|null>(null);
 const [error,setError]=useState('');const [busy,setBusy]=useState(false);
 const load=useCallback(async()=>{try{setStatus(await backend.updates!());setError('');}catch(error){setError(String(error instanceof Error?error.message:error));}},[backend]);
 useEffect(()=>{void load();const timer=setInterval(()=>void load(),1000);return()=>clearInterval(timer);},[load]);
 async function action(action:UpdateAction){setBusy(true);try{setStatus(await backend.updateAction!(action));setError('');}catch(error){setError(String(error instanceof Error?error.message:error));}finally{setBusy(false);}}
 const working=!!status&&['checking','downloading','waiting','installing'].includes(status.phase);
 return <section className="panel updates-panel" aria-labelledby="updates-title">
  <div className="section-heading"><div><div className="eyebrow">APPLICATION</div><h1 id="updates-title">App updates</h1><p>Keep this computer on the latest NodeHarbor version.</p></div></div>
  {error&&<div className="alert" role="alert"><span>{error}</span><button onClick={()=>void load()}>Retry update status</button></div>}
  {!status&&!error&&<p role="status">Loading update status…</p>}
  {status&&<>
   <label className="update-toggle"><input type="checkbox" checked={status.enabled} disabled={busy||status.phase==='installing'} onChange={e=>void action(e.target.checked?'enable':'disable')}/>Automatically install updates</label>
   <p>NodeHarbor checks while the app is running. Updates wait for running jobs, restart the app, and keep your sharing rules.</p>
   <p className="update-message" role={status.phase==='error'?'alert':'status'}>{status.message}</p>
   {status.phase==='downloading'&&<progress aria-label="Update download" max={status.total??undefined} value={status.total?status.downloaded:undefined}/>}
   <p className="muted">Installed: {status.currentVersion}{status.availableVersion&&<> · Available: {status.availableVersion}</>}</p>
   <div className="actions"><button disabled={busy||working} onClick={()=>void action('check')}>Check for updates</button>
    {status.phase==='available'&&<button className="primary" disabled={busy} onClick={()=>void action('install')}>Install update</button>}
    {['downloading','waiting'].includes(status.phase)&&<button disabled={busy} onClick={()=>void action('cancel')}>Cancel update</button>}
   </div>
  </>}
 </section>;
}
