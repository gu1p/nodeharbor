import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { App } from './App';
import { StorageEditor } from './StorageLocations';
import { defaultPolicy, type Backend, type Snapshot, type StoragePlan } from './model';

const location = {id:'disk',volumeId:'external',directory:'/volumes/worker',allocationGib:30,available:true,reason:''};
function snapshot(diskGib=30, revision=1):Snapshot {
 const policy={...defaultPolicy(),resources:{cpus:2,memoryMib:4096,diskGib}};
 return {deviceId:'fixture',name:'Computer',platform:'linux',architecture:'amd64',state:'paused',reason:'Paused',policy,allocatedResources:policy.resources,resources:{cpus:8,memoryMib:16384,diskGib:200},worker:{installed:true,running:false},enrolled:true,controllerUrl:'',version:'test',workloads:[],storage:{supported:true,reason:'',defaultDirectory:'/managed/storage',volumes:[],locations:[{...location,allocationGib:diskGib}],revision,activeGib:diskGib,configuredGib:diskGib}};
}
const deletion:StoragePlan={revision:1,locations:[],totalGib:0,requiresRestart:true,maintenance:{kind:'deleteAll',backup:null,minimumGib:0,temporaryBytes:0,deletions:['All worker data'],downtime:'Storage stays disabled; enrollment remains'}};

it('lets the owner review and confirm deletion of the only missing disk, or cancel safely',async()=>{
 const user=userEvent.setup();
 const applyStorage=vi.fn().mockResolvedValue(snapshot());
 const previewStorage=vi.fn().mockResolvedValue(deletion);
 const inventory={...snapshot().storage!,activeGib:0,locations:[{...location,available:false,reason:'Reconnect the original volume'}],operation:{phase:'missing',message:'Only 0 GiB remains; at least 15 GiB is required'}};
 render(<StorageEditor inventory={inventory} backend={{previewStorage,applyStorage,retryStorageMaintenance:vi.fn(),setStorageRecovery:vi.fn()} as unknown as Backend} disabled={false}/>);
 expect(screen.getByRole('textbox',{name:'Directory for disk 1'})).toBeDisabled();
 expect(screen.getByRole('button',{name:'Add drive'})).toBeDisabled();
 expect(screen.getByRole('button',{name:'Retry storage maintenance'})).toBeEnabled();
 expect(screen.getByRole('button',{name:'Delete all worker storage'})).toBeEnabled();
 await user.click(screen.getByRole('button',{name:'Delete all worker storage'}));
 expect(previewStorage).toHaveBeenCalledWith([],{deleteAll:true});
 expect(screen.getByRole('button',{name:'Confirm deletion'})).toBeDisabled();
 await user.click(screen.getByRole('button',{name:'Cancel storage changes'}));
 expect(applyStorage).not.toHaveBeenCalled();
 await user.click(screen.getByRole('button',{name:'Delete all worker storage'}));
 await user.click(screen.getByRole('checkbox',{name:/I confirm: delete all worker data/i}));
 await user.click(screen.getByRole('button',{name:'Confirm deletion'}));
 expect(applyStorage).toHaveBeenCalledExactlyOnceWith(deletion);
});

it.each(['Drain','Backup','Create','Restore','pending'])('blocks competing deletion during active %s maintenance',phase=>{
 render(<StorageEditor inventory={{...snapshot().storage!,operation:{phase,message:'Working'}}} backend={{previewStorage:vi.fn(),applyStorage:vi.fn()} as unknown as Backend} disabled={false}/>);
 expect(screen.getByRole('button',{name:'Delete all worker storage'})).toBeDisabled();
 expect(screen.getByRole('button',{name:'Review storage changes'})).toBeDisabled();
});

it.each([15,45])('saves the completed %i GiB allowance while preserving unrelated edits through apply and polling',async diskGib=>{
 vi.useFakeTimers();
 try {
  let current=snapshot();
  const pending={...snapshot(30,2),storage:{...snapshot(30,2).storage!,operation:{phase:'Backup',message:'Working'}}};
  const api:Backend={snapshot:vi.fn(async()=>current),savePolicy:vi.fn(async policy=>({...current,policy})),fleet:vi.fn(async()=>[]),action:vi.fn(async()=>current),enroll:vi.fn(async()=>current),previewStorage:vi.fn(async()=>({revision:1,locations:[{...location,allocationGib:diskGib}],totalGib:diskGib,requiresRestart:true})),applyStorage:vi.fn(async()=>{current=pending;return pending;})};
  render(<App backend={api}/>);
  await act(async()=>{});
  await act(async()=>{fireEvent.click(screen.getByRole('button',{name:'Sharing rules'}));});
  fireEvent.change(screen.getByRole('spinbutton',{name:'CPU cores'}),{target:{value:'3'}});
  fireEvent.click(screen.getByRole('checkbox',{name:'Only while idle'}));
  fireEvent.change(screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'}),{target:{value:String(diskGib)}});
  await act(async()=>{fireEvent.click(screen.getByRole('button',{name:'Review storage changes'}));});
  await act(async()=>{fireEvent.click(screen.getByRole('button',{name:'Apply storage changes'}));});
  expect(screen.getByRole('spinbutton',{name:'CPU cores'})).toHaveValue(3);
  current=snapshot(diskGib,3);
  await act(async()=>{vi.advanceTimersByTime(10000);});
  expect(screen.getByRole('spinbutton',{name:'CPU cores'})).toHaveValue(3);
  expect(screen.getByRole('checkbox',{name:'Only while idle'})).toBeChecked();
  await act(async()=>{fireEvent.click(screen.getByRole('button',{name:'Save sharing rules'}));});
  expect(api.savePolicy).toHaveBeenCalledWith(expect.objectContaining({idleOnly:true,resources:{cpus:3,memoryMib:4096,diskGib}}));
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
 } finally {vi.useRealTimers();}
});

it('does not let an older in-flight poll undo completed storage or re-enable sharing after deletion',async()=>{
 vi.useFakeTimers();
 try {
  const before={...snapshot(),policy:{...snapshot().policy,enabled:true}};
  let finishOldPoll!:(s:Snapshot)=>void;
  const deleted={...snapshot(30,3),allocatedResources:null,storage:{...snapshot(30,3).storage!,locations:[],disabled:true,configuredGib:0,activeGib:0}};
  const api:Backend={snapshot:vi.fn().mockResolvedValueOnce(before).mockImplementationOnce(()=>new Promise<Snapshot>(resolve=>{finishOldPoll=resolve;})),savePolicy:vi.fn(async policy=>({...deleted,policy})),fleet:vi.fn(async()=>[]),action:vi.fn(async()=>before),enroll:vi.fn(async()=>before),previewStorage:vi.fn(async()=>deletion),applyStorage:vi.fn(async()=>deleted)};
  render(<App backend={api}/>);await act(async()=>{});
  await act(async()=>{fireEvent.click(screen.getByRole('button',{name:'Sharing rules'}));vi.advanceTimersByTime(10000);});
  fireEvent.change(screen.getByRole('spinbutton',{name:'CPU cores'}),{target:{value:'3'}});
  await act(async()=>{fireEvent.click(screen.getByRole('button',{name:'Delete all worker storage'}));});
  fireEvent.click(screen.getByRole('checkbox',{name:/I confirm: delete all worker data/i}));
  await act(async()=>{fireEvent.click(screen.getByRole('button',{name:'Confirm deletion'}));});
  await act(async()=>{finishOldPoll(before);});
  expect(screen.getByText(/Worker storage is disabled. Configure storage/)).toBeVisible();
  await act(async()=>{fireEvent.click(screen.getByRole('button',{name:'Save sharing rules'}));});
  expect(api.savePolicy).toHaveBeenCalledWith(expect.objectContaining({enabled:false,resources:expect.objectContaining({cpus:3})}));
 } finally {vi.useRealTimers();}
});

it('preserves an edited legacy disk allowance during polling',async()=>{
 vi.useFakeTimers();
 try {
  const legacy={...snapshot(),storage:undefined,allocatedResources:null};
  const api:Backend={snapshot:vi.fn(async()=>legacy),savePolicy:vi.fn(async policy=>({...legacy,policy})),fleet:vi.fn(async()=>[]),action:vi.fn(async()=>legacy),enroll:vi.fn(async()=>legacy)};
  render(<App backend={api}/>);await act(async()=>{});
  fireEvent.click(screen.getByRole('button',{name:'Sharing rules'}));
  fireEvent.change(screen.getByRole('spinbutton',{name:'Disk (GiB)'}),{target:{value:'40'}});
  await act(async()=>{vi.advanceTimersByTime(10000);});
  expect(screen.getByRole('spinbutton',{name:'Disk (GiB)'})).toHaveValue(40);
 } finally {vi.useRealTimers();}
});
