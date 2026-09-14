import * as AlertDialog from '@radix-ui/react-alert-dialog';
import { useState, type RefObject } from 'react';
import type { Policy, StorageInventory, StoragePlan } from './model';
import { policyValues } from './RemoteConfiguration';
export interface SharingReview {plan:StoragePlan;policy:Policy;revision:number|undefined;before:Policy}
export function SharingSaveReview({review,inventory,busy,error,onCancel,onSave,saveButton,onPause,onStop}:{review:SharingReview|null;inventory?:StorageInventory;busy:boolean;error:string;onCancel:()=>void;onSave:()=>void;saveButton:RefObject<HTMLButtonElement|null>;onPause?:()=>void;onStop?:()=>void}) {
 const [confirmStop,setConfirmStop]=useState(false);
 const before=review?policyValues(review.before):{};const after=review?policyValues(review.policy):{};
 return <AlertDialog.Root open={!!review} onOpenChange={open=>{if(!open&&!busy)onCancel();}}><AlertDialog.Portal><AlertDialog.Overlay className="confirmation-overlay"/><AlertDialog.Content className="panel modal confirmation-dialog sharing-review" onCloseAutoFocus={event=>{event.preventDefault();saveButton.current?.focus();}}>
 <AlertDialog.Title>Save sharing rules and storage?</AlertDialog.Title>
 <AlertDialog.Description>{review?.plan.requiresRestart?'Saving these changes drains running work and restarts the worker.':'These settings and storage locations will be used when you prepare the worker.'}</AlertDialog.Description>
 {review&&<><p>{review.policy.resources.cpus} CPU cores · {review.policy.resources.memoryMib/1024} GiB RAM · {review.plan.totalGib} GiB workload storage</p>
 <ul>{review.plan.locations.map(location=><li key={location.id}><strong>{inventory?.volumes.find(v=>v.id===location.volumeId)?.label||'Selected drive'}</strong> · <code>{location.directory}</code> · {location.allocationGib} GiB</li>)}</ul>
 {inventory?.systemDisk&&<p>Separate system disk: {inventory.systemDisk.allocationGib} GiB in <code>{inventory.systemDisk.directory}</code>.</p>}
 <dl>{Object.entries(after).filter(([name,value])=>value!==before[name]).map(([name,value])=><div key={name}><dt>{name}</dt><dd>{before[name]} → {value}</dd></div>)}</dl>
 {review.plan.maintenance&&<><p>{review.plan.maintenance.downtime}</p><p>Temporary backup space: {(review.plan.maintenance.temporaryBytes/1073741824).toFixed(2)} GiB. Minimum resulting capacity: {review.plan.maintenance.minimumGib} GiB.</p>{review.plan.maintenance.backup&&<p>Backup folder: <code>{review.plan.maintenance.backup.directory}</code>. The backup is removed after restore verification.</p>}<ul>{review.plan.maintenance.deletions.map(item=><li key={item}>{item}</li>)}</ul></>}
 </>}
 {onPause&&<div className="actions"><button onClick={onPause}>Pause sharing</button>{onStop&&<button className="danger" onClick={()=>setConfirmStop(true)}>Stop now</button>}</div>}
 {confirmStop&&onStop&&<div><p>Stopping immediately interrupts running work. Running jobs may fail.</p><button onClick={()=>setConfirmStop(false)}>Keep running</button><button className="danger" onClick={()=>{setConfirmStop(false);onStop();}}>Stop and interrupt work</button></div>}
 {error&&<p role="alert">{error}</p>}
 <div className="actions"><AlertDialog.Cancel asChild><button disabled={busy}>Keep editing</button></AlertDialog.Cancel><button className="primary" disabled={busy} aria-busy={busy} onClick={onSave}>{busy?'Saving…':'Confirm and save'}</button></div>
 </AlertDialog.Content></AlertDialog.Portal></AlertDialog.Root>;
}
