import { useEffect, useRef, useState } from 'react';
import * as AlertDialog from '@radix-ui/react-alert-dialog';
import { SharingRules } from './SharingRules';
import type { Backend, ConfigurationOperation, ConfigurationReport, ConfigurationRequest, ConfigurationView, Device, Policy, Snapshot } from './model';

export function RemoteConsent({backend,snapshot,onSaved}:{backend:Backend;snapshot:Snapshot;onSaved:(snapshot:Snapshot)=>void}) {
 const [busy,setBusy]=useState(false);const [error,setError]=useState('');
 if(!backend.setRemoteConsent)return null;
 return <section className="panel" aria-labelledby="remote-consent-title"><h2 id="remote-consent-title">Remote configuration</h2>
 <p id="remote-consent-description">Allow authorized fleet administrators to change CPU, memory, storage, sharing schedules, power rules and workload types on this node. Administrators can add, move, shrink or delete worker disks and allow automatic recovery that discards the entire pool after a missing disk. Resource changes can drain work and restart the worker. Native OS permissions still require local approval. Revoke this permission at any time to cancel unapplied requests.</p>
 <label className="toggle-row"><span>Allow fleet administrators to configure this node</span><input type="checkbox" aria-label="Allow fleet administrators to configure this node" aria-describedby="remote-consent-description" checked={snapshot.configuration?.consent??false} disabled={busy} onChange={async e=>{const enabled=e.target.checked;setBusy(true);setError('');try{onSaved(await backend.setRemoteConsent!(enabled));}catch(e){setError(String(e instanceof Error?e.message:e));}finally{setBusy(false);}}}/></label>
 {error&&<p role="alert">{error}</p>}
 </section>;
}
export function RemoteConfiguration({backend,device,onClose}:{backend:Backend;device:Device;onClose:()=>void}) {
 const [view,setView]=useState<ConfigurationView|null>(null);const [policy,setPolicy]=useState<Policy|null>(null);const [revision,setRevision]=useState<number|null>(null);
 const [error,setError]=useState('');const [loadError,setLoadError]=useState('');const [busy,setBusy]=useState(false);const [confirm,setConfirm]=useState(false);
 const mounted=useRef(true);const storageRetries=useRef(new Map<string,string>());
 useEffect(()=>{mounted.current=true;return()=>{mounted.current=false;};},[]);
 const retry=useRef<{key:string;id:string}|null>(null);const reviewButton=useRef<HTMLButtonElement>(null);
 useEffect(()=>{let active=true;let loading=false;const refresh=async()=>{if(loading)return;loading=true;try{const result=await backend.configuration!(device.deviceId);if(active){setView(result);setPolicy(p=>p??result.report?.policy??null);setRevision(r=>r??result.report?.revision??null);setLoadError('');}}catch(e){if(active)setLoadError(String(e instanceof Error?e.message:e));}finally{loading=false;}};void refresh();const timer=setInterval(()=>void refresh(),10000);return()=>{active=false;clearInterval(timer);};},[backend,device.deviceId]);
 async function reload(){setBusy(true);try{const result=await backend.configuration!(device.deviceId);setView(result);setPolicy(result.report?.policy??null);setRevision(result.report?.revision??null);setError('');setLoadError('');retry.current=null;}catch(e){setLoadError(String(e instanceof Error?e.message:e));}finally{setBusy(false);}}
 async function submit(){if(!policy||revision===null)return;setBusy(true);setError('');const key=JSON.stringify({policy,revision});if(retry.current?.key!==key)retry.current={key,id:crypto.randomUUID()};try{const result=await backend.configureDevice!(device.deviceId,{requestId:retry.current.id,expectedRevision:revision,policy,acknowledgeInterruption:true});setView(v=>v?{...v,requests:[result,...v.requests.filter(r=>r.requestId!==result.requestId)]}:v);}catch(e){setError(String(e instanceof Error?e.message:e));}finally{setBusy(false);}}
 function storageSnapshot(current:ConfigurationReport):Snapshot {
  return {deviceId:device.deviceId,name:device.name,platform:device.platform,architecture:device.architecture,state:device.state,reason:device.reason,policy:current.policy!,resources:current.hardware!,storage:current.storage??undefined,configuration:current,allocatedResources:current.allocatedResources,worker:{installed:!!current.allocatedResources,running:device.state==='sharing'},enrolled:true,controllerUrl:'',version:'',workloads:[]};
 }
 async function storageRequest(operation:ConfigurationOperation):Promise<ConfigurationRequest> {
  if(!view?.online||!view.report?.consent||!view.report.policy||revision!==view.report.revision)throw new Error('Reload current settings and check owner consent before configuring storage');
  const key=JSON.stringify({operation,revision});const requestId=storageRetries.current.get(key)??crypto.randomUUID();storageRetries.current.set(key,requestId);
  let request=await backend.configureDevice!(device.deviceId,{requestId,expectedRevision:revision,policy:view.report.policy,acknowledgeInterruption:operation.type!=='storagePreview',operation});
  if(mounted.current)setView(v=>v?{...v,requests:[request,...v.requests.filter(r=>r.requestId!==request.requestId)]}:v);
  if(operation.type==='storagePreview') {
   const deadline=Date.now()+120000;
   while(request.status==='requested'||request.status==='pending') {
    await new Promise(resolve=>setTimeout(resolve,1000));
    if(!mounted.current)throw new Error('Storage review closed; its result remains in configuration history');
    const next=await backend.configuration!(device.deviceId);setView(next);setLoadError('');
    request=next.requests.find(r=>r.requestId===requestId)??request;
    if(!next.online)throw new Error('Node offline. The review remains in history; retry after it reconnects.');
    if(Date.now()>=deadline)throw new Error('Node review is still pending. Retry this review to retrieve its acknowledgment.');
   }
  }
  if(request.status==='rejected'){storageRetries.current.delete(key);throw new Error(request.error??'The node rejected this storage request');}
  storageRetries.current.delete(key);
  return request;
 }
 const storageBackend:Backend={...backend,
  previewStorage:async(selections,options={})=>{const result=await storageRequest({type:'storagePreview',selections,options});if(!result.result)throw new Error('The node did not return a storage plan; review again');return result.result;},
  applyStorage:async plan=>{await storageRequest({type:'storageApply',plan});return storageSnapshot(view!.report!);},
  setStorageRecovery:async enabled=>{await storageRequest({type:'storageRecovery',enabled});return storageSnapshot(view!.report!);},
  retryStorageMaintenance:async()=>{await storageRequest({type:'storageRetry'});return storageSnapshot(view!.report!);},
  chooseStorageDirectory:undefined,
 };
 const report=view?.report;const pending=view?.requests.some(r=>r.status==='requested'||r.status==='pending')??false;
 const locked=!view?.online||!report?.consent||pending||!!loadError||revision!==report?.revision;
 return <section aria-label={`Configure ${device.name}`} className="configuration-editor">
 <div className="section-heading"><h2>Configure {device.name}</h2><button onClick={onClose}>Back to fleet</button></div>
 {error&&<p role="alert">{error}</p>}{loadError&&<p role="alert">{loadError}</p>}{busy&&<p role="status">Contacting the controller…</p>}
 {!view&&!loadError&&<p role="status">Loading node configuration…</p>}
 {view&&<><p role="status">{!view.online?'Offline: current capacity cannot be verified. Reconnect the node before applying changes.':!report?'This node has not reported remote configuration support. Configure it locally; remote configuration is unavailable on this agent.':!report.consent?'The local owner has not allowed remote configuration. Enable it in the desktop Sharing rules.':pending?'A configuration request is pending node acknowledgment.':'Remote configuration is allowed by the local owner.'}</p>
 {report&&revision!==report.revision&&<p role="alert">Settings changed locally or remotely. Reload current settings before submitting another edit.</p>}
 <button disabled={busy} onClick={()=>void reload()}>Reload current settings</button>
 {report?.hardware&&<section className="panel" aria-label="Hardware and storage inventory"><h3>Hardware and available capacity</h3><p>{report.hardware.cpus} CPU cores · {Math.round(report.hardware.memoryMib/1024)} GiB RAM · {report.hardware.diskGib} GiB disk capacity including the worker allocation</p>
 <p>Leave one CPU core, 2 GiB RAM and 10 GiB disk capacity for the host. The node checks capacity again before applying.</p>
 {report.allocatedResources?<p>Effective worker allocation: {report.allocatedResources.cpus} CPU · {report.allocatedResources.memoryMib/1024} GiB RAM · {report.allocatedResources.diskGib} GiB disk</p>:<p>No verified worker allocation reported.</p>}
 {report.workerDiskLocation&&<p>Worker disk location: {report.workerDiskLocation}</p>}
 <ul>{report.storageInventory.map((disk,i)=><li key={i}>{disk.label} · {disk.mountPoint} · {disk.filesystem} · {disk.availableGib} GiB available{disk.configuredGib!==undefined&&` · ${disk.configuredGib} GiB configured`}{disk.reason&&` · ${disk.reason}`}</li>)}</ul>
 <p>{report.capabilities.storageReason}</p>{report.capabilities.diskGrowthReason&&<p>{report.capabilities.diskGrowthReason}</p>}<p>Start at login requires local approval in the desktop application.</p></section>}
 {policy&&report?.hardware?<SharingRules policy={policy} host={report.hardware} storage={report.storage??undefined} storageBackend={report.capabilities.storage?storageBackend:undefined} onChange={setPolicy} onSave={()=>setConfirm(true)} busy={busy} locked={locked} remote diskEditable={report.capabilities.diskGrowth!==false} saveLabel="Review configuration" saveButtonRef={reviewButton}/>:<button disabled>Review configuration</button>}
 <section className="panel" aria-label="Configuration history"><h3>Configuration history</h3>{view.requests.length===0?<p>No configuration requests recorded.</p>:<ol>{view.requests.map(request=><li key={request.requestId}><p role="status"><strong>{request.operation?.type==='storagePreview'&&request.status==='applied'?'Reviewed':request.status.charAt(0).toUpperCase()+request.status.slice(1)}</strong> · {request.actor??'Administrator'} · {request.requestedAt?new Date(request.requestedAt).toLocaleString():request.requestId}{request.acknowledgedAt&&` · Node acknowledged ${new Date(request.acknowledgedAt).toLocaleString()}`}</p>{request.error&&<p>{request.error}</p>}{request.operation&&<StorageAudit request={request}/>} {request.policy&&!request.operation&&<details><summary>Requested settings and effective values</summary><SettingsAudit request={request}/></details>}</li>)}</ol>}</section>
 </>}
 <AlertDialog.Root open={confirm} onOpenChange={setConfirm}><AlertDialog.Portal><AlertDialog.Overlay className="confirmation-overlay"/><AlertDialog.Content className="panel modal confirmation-dialog" onCloseAutoFocus={e=>{e.preventDefault();reviewButton.current?.focus();}}><AlertDialog.Title>Apply node configuration?</AlertDialog.Title><AlertDialog.Description>The worker will drain running work before applying configuration and may restart. Work still running after the current owner drain deadline of {report?.policy?.drainSeconds??0} seconds may be interrupted. Sharing still requires owner consent, VM isolation and fleet health qualification. Host and VPN settings remain under local control.</AlertDialog.Description><div className="actions"><AlertDialog.Cancel asChild><button>Keep editing</button></AlertDialog.Cancel><AlertDialog.Action asChild><button disabled={busy||locked} onClick={()=>void submit()}>Apply configuration</button></AlertDialog.Action></div></AlertDialog.Content></AlertDialog.Portal></AlertDialog.Root>
 </section>;
}

export function policyValues(policy:Policy|undefined|null):Record<string,string> {
 if(!policy)return {};
 const yes=(value:boolean)=>value?'Yes':'No';
 return {'Enable sharing':yes(policy.enabled),'CPU cores':String(policy.resources.cpus),'RAM (GiB)':String(policy.resources.memoryMib/1024),'Disk (GiB)':String(policy.resources.diskGib),'Only while idle':yes(policy.idleOnly),'Idle time (minutes)':String(policy.idleAfterMinutes),'Allow sharing on battery':yes(policy.allowBattery),'Minimum battery (%)':String(policy.minBatteryPercent),'Use a weekly schedule':yes(policy.scheduleEnabled),'Schedule':policy.schedule.map(w=>`${w.days.map(d=>['Mon','Tue','Wed','Thu','Fri','Sat','Sun'][d]).join(', ')} ${formatMinute(w.startMinute)}–${formatMinute(w.endMinute)}`).join('; ')||'No windows','Start at login':yes(policy.startAtLogin),'Continue when the window closes':yes(policy.background),'Accept CI jobs':yes(policy.allowCi),'Accept eligible services':yes(policy.allowServices),'Drain deadline (seconds)':String(policy.drainSeconds)};
}
function formatMinute(minute:number){return `${String(Math.floor(minute/60)).padStart(2,'0')}:${String(minute%60).padStart(2,'0')}`;}
function SettingsAudit({request}:{request:ConfigurationRequest}) {
 const before=policyValues(request.beforePolicy);const requested=policyValues(request.policy);const effective=policyValues(request.effectivePolicy);
 const changes=Object.keys(requested).filter(key=>requested[key]!==before[key]);
 return <>{changes.length?<table aria-label={`Settings changed by ${request.actor??'Administrator'}`}><thead><tr><th scope="col">Setting</th><th scope="col">Previous</th><th scope="col">Requested</th><th scope="col">Node acknowledgment</th></tr></thead><tbody>{changes.map(key=><tr key={key}><th scope="row">{key}</th><td>{before[key]??'Unavailable'}</td><td>{requested[key]}</td><td>{effective[key]??'Awaiting acknowledgment'}</td></tr>)}</tbody></table>:<p>No setting values changed.</p>}{request.effectiveResources&&<p>Verified allocation: {request.effectiveResources.cpus} CPU · {request.effectiveResources.memoryMib/1024} GiB RAM · {request.effectiveResources.diskGib} GiB disk</p>}</>;
}

function StorageAudit({request}:{request:ConfigurationRequest}) {
 const operation=request.operation!;
 const names={storagePreview:'Storage review',storageApply:'Storage change',storageRecovery:'Automatic storage recovery',storageRetry:'Storage maintenance retry'};
 const locations=operation.type==='storageApply'?operation.plan.locations:operation.type==='storagePreview'?operation.selections:[];
 const before=request.beforeStorage;const effective=request.effectiveStorage;
 const capacity=(value:number|undefined)=>value===undefined?'Unavailable':`${value} GiB`;
 const recovery=(value:boolean|undefined)=>value===undefined?'Unavailable':value?'Enabled':'Disabled';
 const rows:[string,string,string,string][]=[];
 if(operation.type==='storageRecovery')rows.push(['Automatic recovery',recovery(before?.recoveryEnabled),recovery(operation.enabled),effective?recovery(effective.recoveryEnabled):'Awaiting acknowledgment']);
 if(operation.type==='storageApply') {
  rows.push(['Configured storage',capacity(before?.configuredGib),capacity(operation.plan.totalGib),effective?capacity(effective.configuredGib):'Awaiting acknowledgment']);
  operation.plan.locations.forEach((location,index)=>{
   const previous=before?.locations.find(l=>l.id===location.id||l.directory===location.directory);
   const applied=effective?.locations.find(l=>l.id===location.id||l.directory===location.directory);
   rows.push([`Disk ${index+1} directory`,previous?.directory??'New disk',location.directory,applied?.directory??'Not acknowledged']);
   rows.push([`Disk ${index+1} allocation`,previous?capacity(previous.allocationGib):'New disk',capacity(location.allocationGib),applied?capacity(applied.allocationGib):'Not acknowledged']);
  });
  before?.locations.filter(l=>!operation.plan.locations.some(target=>target.id===l.id||target.directory===l.directory)).forEach(location=>rows.push(['Removed disk',`${location.directory} · ${capacity(location.allocationGib)}`,'Remove from worker',effective&&!effective.locations.some(l=>l.id===location.id||l.directory===location.directory)?'Removed':'Not acknowledged']));
 }
 return <details><summary>{names[operation.type]} and effective values</summary>
 {operation.type==='storagePreview'&&<p>Read-only review; no settings were changed.</p>}
 {operation.type==='storageRecovery'&&<p>Recovery may discard all worker data after a missing disk.</p>}
 {!!rows.length&&<table aria-label={`Storage changed by ${request.actor??'Administrator'}`}><thead><tr><th scope="col">Setting</th><th scope="col">Previous</th><th scope="col">Requested</th><th scope="col">Node acknowledgment</th></tr></thead><tbody>{rows.map(([setting,previous,requested,applied],i)=><tr key={i}><th scope="row">{setting}</th><td>{previous}</td><td>{requested}</td><td>{applied}</td></tr>)}</tbody></table>}
 {!!locations.length&&operation.type==='storagePreview'&&<ul>{locations.map((location,i)=><li key={i}>{location.directory} · {location.allocationGib} GiB reviewed</li>)}</ul>}
 {operation.type==='storageApply'&&operation.plan.maintenance?.kind==='deleteAll'&&<p>Requested deletion of all worker storage.</p>}
 {effective&&<><p>Node acknowledgment: {effective.configuredGib??0} GiB configured · {effective.activeGib??0} GiB active · automatic recovery {effective.recoveryEnabled?'enabled':'disabled'}.</p>{effective.operation&&<p>{effective.operation.message}</p>}</>}
 </details>;
}
