import { useId, useState, useRef, useEffect } from 'react';
import type { Backend, Snapshot, StorageInventory, StoragePlan, StorageSelection, StorageReviewOptions } from './model';

export function StorageLocations({inventory,locations,onChange,onChoose,onDeleteAll,disabled=false,deleteDisabled=disabled}:{
 inventory:StorageInventory; locations:StorageSelection[];
 onChange?:(locations:StorageSelection[])=>void; onChoose?:(index:number)=>void; disabled?:boolean; deleteDisabled?:boolean; onDeleteAll?:()=>void;
}) {
 const title=useId();const impact=useId();const focusNew=useRef(false);const drives=useRef<(HTMLSelectElement|null)[]>([]);
 useEffect(()=>{if(focusNew.current){drives.current[locations.length-1]?.focus();focusNew.current=false;}},[locations.length]);
 const editable=inventory.supported&&!!onChange&&!disabled;
 const change=(index:number,patch:Partial<StorageSelection>)=>onChange?.(locations.map((location,i)=>i===index?{...location,...patch}:location));
 return <section className="panel" aria-labelledby={title}>
  <h2 id={title}>Storage locations</h2>
  {inventory.defaultDirectory&&<p>The application-managed worker directory is <code>{inventory.defaultDirectory}</code>.</p>}
  {inventory.systemDisk&&<p>The worker system disk uses a separate {inventory.systemDisk.allocationGib} GiB allowance in <code>{inventory.systemDisk.directory}</code>. This does not add workload storage.</p>}
  {!inventory.supported&&<p>{inventory.reason}</p>}
  {inventory.volumes.length===0?<p>No host volumes could be inspected.</p>:<ul>
   {inventory.volumes.map((volume,index)=><li key={`${volume.id}:${volume.mountPoint}:${index}`}>
    <strong>{volume.label||'Unnamed volume'}</strong> · <code>{volume.mountPoint}</code>
    <p>{volume.filesystem||'Unknown filesystem'} · {volume.driveType?volume.driveType.toUpperCase():'Drive type unknown'} · {volume.availableGib} GiB available · {volume.configuredGib} GiB configured</p>
    {!volume.eligible&&<p>{volume.reason}</p>}
   </li>)}
  </ul>}
  {inventory.locations.map(location=><div key={location.id}>
   <p><code>{location.directory}</code> · {location.allocationGib} GiB configured</p>
   {!location.available&&<p role="alert">{location.reason}</p>}
  </div>)}
  <p>{locations.reduce((total,location)=>total+location.allocationGib,0)||(inventory.disabled?0:inventory.configuredGib||30)} GiB allocated. Usable Kubernetes storage will be less because filesystem space and worker reserves are reserved.</p>
  <p id={impact}>Storage changes require a drain and worker restart. Running work may be interrupted at your drain deadline. Shrink allocation and Remove disk preserve worker data through verified backup and restore. Temporary backup space is required. Delete all worker storage permanently deletes worker data.</p>
  <p>Disconnecting any selected disk makes storage unavailable for the whole worker. Combining disks does not provide a backup.</p><p>Recommended during setup: review automatic recovery below and decide whether to allow whole-pool data loss after a missing disk.</p>
  {inventory.operation&&<p role="status">{inventory.operation.message}</p>}
  {!!inventory.retainedCopies?.length&&<div><h3>Previous storage copies</h3><p>These copies occupy space without adding worker capacity. Returned stale disks remain excluded until you restore their capacity. To finish deleting all storage, reconnect their original volumes and use Delete all worker storage again.</p><ul>{inventory.retainedCopies.map(copy=><li key={`${copy.id}:${copy.directory}`}><code>{copy.directory}</code> · previous {copy.allocationGib} GiB disk{!copy.available&&<p>{copy.reason}</p>}</li>)}</ul></div>}
  <fieldset disabled={!editable} className="rules-fields" aria-describedby={impact}>
   {locations.map((location,index)=><fieldset key={index}>
    <legend>Disk {index+1}</legend>
    <label>Drive for disk {index+1}<select required ref={element=>{drives.current[index]=element;}} value={location.expectedVolumeId??inventory.locations.find(saved=>saved.id===location.id)?.volumeId??''} onChange={event=>{const volume=inventory.volumes.find(volume=>volume.id===event.target.value);change(index,{expectedVolumeId:event.target.value,directory:volume?.suggestedDirectory??''});}}>
     <option value="">Select a drive</option>
     {inventory.volumes.filter(volume=>volume.id).map((volume,i)=><option key={`${volume.id}:${i}`} value={volume.id} disabled={!volume.eligible}>{volume.label||'Unnamed volume'} · {volume.mountPoint} · {volume.filesystem} · {volume.availableGib} GiB available</option>)}
     {location.expectedVolumeId&&!inventory.volumes.some(volume=>volume.id===location.expectedVolumeId)&&<option value={location.expectedVolumeId}>Unavailable saved drive</option>}
    </select></label>
    <label>Allocation for disk {index+1} (GiB)<input type="number" id={`${title}-allocation-${index}`} min={1} max={1048576} step={1} required value={location.allocationGib||''} onChange={event=>change(index,{allocationGib:Number(event.target.value)})}/></label>
    <label>Directory for disk {index+1}<input type="text" required value={location.directory} onChange={event=>change(index,{directory:event.target.value})}/></label>
    <p>Customize the folder within the selected drive. If no folder is suggested, choose a writable folder before reviewing.</p>
    {onChoose&&<button type="button" onClick={()=>onChoose(index)}>Choose folder for disk {index+1}</button>}
    {inventory.locations.some(saved=>saved.id===location.id)&&<button type="button" onClick={()=>document.getElementById(`${title}-allocation-${index}`)?.focus()}>Shrink allocation for disk {index+1}</button>}
    {(locations.length>1||!inventory.locations.some(saved=>saved.id===location.id))&&<button type="button" onClick={()=>onChange?.(locations.filter((_,i)=>i!==index))}>Remove disk {index+1}</button>}
   </fieldset>)}
   <button type="button" disabled={locations.length>=16} onClick={()=>{focusNew.current=true;onChange?.([...locations,{directory:'',allocationGib:30}]);}}>Add drive</button>
  </fieldset>
  {!!(inventory.locations.length||inventory.retainedCopies?.length)&&<button type="button" disabled={deleteDisabled||(!onDeleteAll&&!editable)} onClick={onDeleteAll??(()=>onChange?.([]))}>Delete all worker storage</button>}
 </section>;
}

export function StorageEditor({inventory,backend,onSaved,disabled}:{inventory:StorageInventory;backend?:Backend;onSaved?:(snapshot:Snapshot)=>void;disabled:boolean}) {
 const [locations,setLocations]=useState<StorageSelection[]>(()=>inventory.locations.map(({id,directory,allocationGib,volumeId})=>({id,directory,allocationGib,expectedVolumeId:volumeId})));
 const [plan,setPlan]=useState<StoragePlan|null>(null);const [busy,setBusy]=useState(false);const [error,setError]=useState('');
 const [confirmed,setConfirmed]=useState(false);const [temporaryDirectory,setTemporaryDirectory]=useState('');const [singleDiskGib,setSingleDiskGib]=useState(inventory.configuredGib||30);
 const available=!!backend?.previewStorage&&!!backend?.applyStorage;
 const editable=available&&inventory.supported;
 const blocked=disabled||busy||!!inventory.operation;
 const deleteBlocked=disabled||busy||!available||!!(inventory.operation&&inventory.operation.phase!=='missing');
 const change=(next:StorageSelection[])=>{setLocations(next);setPlan(null);setError('');setConfirmed(false);};
 async function review(options?:StorageReviewOptions){setBusy(true);setError('');setPlan(null);setConfirmed(false);try{
  const selected=options?.deleteAll?[]:locations;
  if(inventory.supported&&!options?.deleteAll&&!options?.restoreDisk&&selected.some(location=>!location.expectedVolumeId||!location.directory.trim()))throw new Error('Select a drive and a writable folder for every disk before review.');
  const request={...(temporaryDirectory?{temporaryDirectory}:{}),...options};
  setPlan(await (Object.keys(request).length?backend!.previewStorage!(selected,request):backend!.previewStorage!(selected)));
 }catch(error){setError(String(error instanceof Error?error.message:error));}finally{setBusy(false);}}
 async function apply(){if(!plan||(plan.maintenance?.kind==='deleteAll'&&!confirmed))return;setBusy(true);setError('');try{const snapshot=await backend!.applyStorage!(plan);setPlan(null);onSaved?.(snapshot);}catch(error){setError(String(error instanceof Error?error.message:error));}finally{setBusy(false);}}
 async function choose(index:number){setBusy(true);setError('');try{const directory=await backend!.chooseStorageDirectory!();if(directory)change(locations.map((location,i)=>i===index?{...location,directory}:location));}catch(error){setError(String(error instanceof Error?error.message:error));}finally{setBusy(false);}}
 async function recovery(enabled:boolean){setBusy(true);setError('');try{onSaved?.(await backend!.setStorageRecovery!(enabled));}catch(error){setError(String(error));}finally{setBusy(false);}}
 async function retry(){setBusy(true);setError('');try{onSaved?.(await backend!.retryStorageMaintenance!());}catch(error){setError(String(error));}finally{setBusy(false);}}
 return <><StorageLocations inventory={inventory} locations={locations} onChange={editable?change:undefined} onChoose={backend?.chooseStorageDirectory?index=>void choose(index):undefined} onDeleteAll={()=>void review({deleteAll:true})} disabled={blocked} deleteDisabled={deleteBlocked}/>
 <section className="panel" aria-label="Storage recovery">
 <p>{inventory.activeGib??inventory.locations.reduce((n,l)=>n+l.allocationGib,0)} GiB active · {inventory.configuredGib??inventory.locations.reduce((n,l)=>n+l.allocationGib,0)} GiB configured</p>
 {inventory.disabled&&<p role="status">Worker storage is disabled. Configure storage before preparing this worker again. Host enrollment remains.</p>}
 <label><input type="checkbox" checked={inventory.recoveryEnabled??false} disabled={disabled||busy||!backend?.setStorageRecovery} onChange={event=>void recovery(event.target.checked)}/>Automatically recover after a missing disk</label>
 <p>Recommended if you accept data loss: after two minutes, recovery can discard local data from the entire old pool and rebuild on remaining selected disks. Pause and Stop override recovery. Existing installations keep waiting until you enable this setting.</p>
 <p>Workloads restart according to their Kubernetes retry policies. Every interrupted job is not guaranteed to retry.</p>
 {inventory.excluded?.map(location=><div key={location.id}><p><code>{location.directory}</code> remains excluded. Its stale managed storage must be replaced to restore capacity.</p><button type="button" disabled={blocked||!available||!location.available} onClick={()=>void review({restoreDisk:location.id})}>Restore this disk’s capacity</button></div>)}
 {inventory.backupCleanup?.map(backup=><p key={backup.path} role="status">Backup cleanup pending: <code>{backup.path}</code> still occupies {(backup.bytes/1073741824).toFixed(2)} GiB. Reconnect the backup volume to finish cleanup.</p>)}
 {inventory.recoveryBackup&&<p role="status">Worker recovery backup: <code>{inventory.recoveryBackup.path}</code> · {(inventory.recoveryBackup.bytes/1073741824).toFixed(2)} GiB. Keep this file until storage recovery and verification finish. {inventory.recoveryBackup.verified?'Backup verified.':'Backup verification has not finished.'}</p>}
 {inventory.operation&&backend?.retryStorageMaintenance&&<button type="button" disabled={disabled||busy} onClick={()=>void retry()}>Retry storage maintenance</button>}
 </section>
 {error&&<p role="alert">{error}</p>}
 {available&&<div className="panel">
 {!inventory.supported&&<><label>Worker disk allocation (GiB)<input type="number" min={15} max={1048576} value={singleDiskGib} disabled={blocked} onChange={e=>{setSingleDiskGib(Number(e.target.value));setPlan(null);}}/></label><button type="button" disabled={blocked} onClick={()=>void review({singleDiskGib})}>Shrink allocation</button><button type="button" disabled={blocked} onClick={()=>void review({deleteAll:true})}>Delete all worker storage</button></>}
 <label>Temporary backup folder (optional)<input type="text" value={temporaryDirectory} disabled={blocked} onChange={e=>{setTemporaryDirectory(e.target.value);setPlan(null);}}/></label>
 <p>NodeHarbor prefers sufficient space on a selected volume. If necessary, choose another temporary folder. Backups remain private and are never extracted on this computer.</p>
 {editable&&<button type="button" disabled={blocked} onClick={()=>void review()}>Review storage changes</button>}
 {plan&&<div role="region" aria-label="Storage change review"><p role="status">Reviewed: {plan.totalGib} GiB allocated.{plan.requiresRestart?' Applying this change drains work and restarts the worker.':' These locations will be used when you prepare the worker.'}</p>
 <ul>{plan.locations.map(location=><li key={location.id}><code>{location.directory}</code> · {location.allocationGib} GiB</li>)}</ul>
 {plan.maintenance&&<><p>Downtime: {plan.maintenance.downtime}</p><p>Temporary space: {(plan.maintenance.temporaryBytes/1073741824).toFixed(2)} GiB. Minimum resulting capacity: {plan.maintenance.minimumGib} GiB.</p>{plan.maintenance.backup&&<p>Backup location: <code>{plan.maintenance.backup.directory}</code>. The backup is removed after restore verification; unfinished cleanup remains visible.</p>}<ul>{plan.maintenance.deletions.map(item=><li key={item}>{item}</li>)}</ul></>}
 {plan.maintenance?.kind==='deleteAll'&&<><p>Host enrollment remains. All worker data is permanently deleted and storage stays disabled.</p><label><input type="checkbox" checked={confirmed} onChange={e=>setConfirmed(e.target.checked)}/>I confirm: delete all worker data</label></>}
 <button type="button" disabled={disabled||busy||(plan.maintenance?.kind==='deleteAll'&&!confirmed)} onClick={()=>void apply()}>{plan.maintenance?.kind==='deleteAll'?'Confirm deletion':'Apply storage changes'}</button><button type="button" disabled={busy} onClick={()=>setPlan(null)}>Cancel storage changes</button></div>}
 </div>}</>;
}
