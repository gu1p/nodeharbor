import { render, screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { App } from './App';
import { defaultPolicy, type Backend, type Policy, type Snapshot, type StoragePlan } from './model';

function fixture() {
 let current:Snapshot={deviceId:'save-fixture',name:'Computer',platform:'linux',architecture:'amd64',state:'paused',reason:'Paused',policy:defaultPolicy(),resources:{cpus:8,memoryMib:16384,diskGib:27},enrolled:true,controllerUrl:'',version:'test',workloads:[],worker:{installed:false,running:false},storage:{revision:1,supported:true,reason:'',defaultDirectory:'/managed/storage',locations:[],systemDisk:{directory:'/managed',allocationGib:16},volumes:[{id:'data',label:'Data SSD',mountPoint:'/data',filesystem:'ext4',availableGib:200,configuredGib:0,eligible:true,reason:'',suggestedDirectory:'/data/NodeHarbor'}]},configuration:{revision:4,consent:false,policy:defaultPolicy(),hardware:null,allocatedResources:null,storageInventory:[],receipts:[],capabilities:{storage:true,storageReason:'',localApproval:[]}}};
 const plan:StoragePlan={revision:1,totalGib:100,requiresRestart:false,locations:[{id:'one',volumeId:'data',directory:'/data/NodeHarbor',allocationGib:100}]};
 const api:Backend={snapshot:vi.fn(async()=>current),fleet:vi.fn(async()=>[]),action:vi.fn(async()=>current),enroll:vi.fn(async()=>current),previewStorage:vi.fn(async()=>plan),applyStorage:vi.fn(async()=>current),savePolicy:vi.fn(async(policy:Policy,_revision?:number,storage?:StoragePlan)=>{current={...current,policy,configuration:{...current.configuration!,revision:5,policy},storage:{...current.storage!,revision:2,locations:(storage?.locations??[]).map(l=>({...l,available:true,reason:''}))}};return current;})};
 return {api,plan,setSnapshot:(s:Snapshot)=>{current=s;},snapshot:()=>current};
}
async function edit(api:Backend) {
 const user=userEvent.setup();const mounted=render(<App backend={api}/>);
 await user.click(await screen.findByRole('button',{name:'Sharing rules'}));
 await user.clear(screen.getByRole('spinbutton',{name:'CPU cores'}));await user.type(screen.getByRole('spinbutton',{name:'CPU cores'}),'3');
 await user.click(screen.getByRole('checkbox',{name:'Only while idle'}));
 await user.click(screen.getByRole('button',{name:'Add drive'}));
 await user.selectOptions(screen.getByRole('combobox',{name:'Drive for disk 1'}),'data');
 await user.clear(screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'}));await user.type(screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'}),'100');
 return {user,mounted};
}
it('saves a selected 100 GiB disk and sharing rules together, then reopens and prepares',async()=>{
 const {api,plan}=fixture();const {user,mounted}=await edit(api);
 const save=screen.getByRole('button',{name:'Save sharing rules'});expect(save).toBeEnabled();
 await user.click(save);
 const review=await screen.findByRole('alertdialog',{name:'Save sharing rules and storage?'});
 expect(review).toHaveTextContent('100 GiB');expect(review).toHaveTextContent('/data/NodeHarbor');expect(review).toHaveTextContent('3 CPU');
 expect(within(review).getByRole('button',{name:'Keep editing'})).toHaveFocus();
 expect(api.savePolicy).not.toHaveBeenCalled();
 await user.click(within(review).getByRole('button',{name:'Confirm and save'}));
 await waitFor(()=>expect(api.savePolicy).toHaveBeenCalledWith(expect.objectContaining({idleOnly:true,resources:{cpus:3,memoryMib:4096,diskGib:100}}),4,plan));
 expect(api.applyStorage).not.toHaveBeenCalled();
 expect(await screen.findByText('Sharing rules and storage saved')).toBeVisible();
 mounted.unmount();render(<App backend={api}/>);
 await user.click(await screen.findByRole('button',{name:'Sharing rules'}));
 expect(screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'})).toHaveValue(100);
 expect(screen.getByRole('spinbutton',{name:'CPU cores'})).toHaveValue(3);
 await user.click(screen.getByRole('button',{name:'Your machine'}));await user.click(screen.getByRole('button',{name:'Prepare worker'}));
 expect(api.action).toHaveBeenCalledWith('prepare');
});
it('preserves disk, folder, backup and policy drafts through navigation and canceled review',async()=>{
 const {api}=fixture();const {user}=await edit(api);
 await user.type(screen.getByRole('textbox',{name:'Temporary backup folder (optional)'}),'/backup');
 await user.click(screen.getByRole('button',{name:'Your machine'}));await user.click(screen.getByRole('button',{name:'Sharing rules'}));
 expect(screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'})).toHaveValue(100);
 expect(screen.getByRole('textbox',{name:'Temporary backup folder (optional)'})).toHaveValue('/backup');
 await user.click(screen.getByRole('button',{name:'Save sharing rules'}));await user.click(await screen.findByRole('button',{name:'Keep editing'}));
 expect(screen.getByRole('button',{name:'Save sharing rules'})).toHaveFocus();
 expect(screen.getByRole('spinbutton',{name:'CPU cores'})).toHaveValue(3);
 expect(api.savePolicy).not.toHaveBeenCalled();
});
it('returns preparation to the unsaved draft instead of preparing the old allocation',async()=>{
 const {api}=fixture();const {user}=await edit(api);
 await user.click(screen.getByRole('button',{name:'Your machine'}));await user.click(screen.getByRole('button',{name:'Prepare worker'}));
 expect(api.action).not.toHaveBeenCalled();
 expect(screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'})).toHaveValue(100);
 expect(screen.getByText(/save your sharing rules and storage before preparing/i)).toBeVisible();
});
it('keeps rejected save drafts editable and never reports success',async()=>{
 const {api}=fixture();api.savePolicy=vi.fn().mockRejectedValue(new Error('Selected drive disappeared; reconnect Data SSD'));
 const {user}=await edit(api);await user.click(screen.getByRole('button',{name:'Save sharing rules'}));await user.click(await screen.findByRole('button',{name:'Confirm and save'}));
 expect(await screen.findByRole('alert')).toHaveTextContent('Selected drive disappeared');
 await user.click(screen.getByRole('button',{name:'Keep editing'}));
 expect(screen.getByRole('button',{name:'Save sharing rules'})).toBeEnabled();
 expect(screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'})).toHaveValue(100);
 expect(screen.queryByText('Sharing rules and storage saved')).not.toBeInTheDocument();
});
it('preserves drafts but rejects a review after another process changes storage',async()=>{
 const f=fixture();const {user}=await edit(f.api);
 f.setSnapshot({...f.snapshot(),storage:{...f.snapshot().storage!,revision:2},configuration:{...f.snapshot().configuration!,revision:5}});
 await user.click(screen.getByRole('button',{name:'Your machine'}));await user.click(screen.getByRole('button',{name:'Refresh machine status'}));
 await user.click(screen.getByRole('button',{name:'Sharing rules'}));await user.click(screen.getByRole('button',{name:'Save sharing rules'}));
 expect(await screen.findByRole('alert')).toHaveTextContent(/changed.*reload/i);
 expect(f.api.savePolicy).not.toHaveBeenCalled();
 expect(screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'})).toHaveValue(100);
});
it('prevents duplicate commits while keeping the running worker pause control available',async()=>{
 const f=fixture();f.setSnapshot({...f.snapshot(),worker:{installed:true,running:true},policy:{...f.snapshot().policy,enabled:true}});
 let reject!:(error:Error)=>void;
 f.api.savePolicy=vi.fn(()=>new Promise<Snapshot>((_,r)=>{reject=r;}));
 const {user}=await edit(f.api);await user.click(screen.getByRole('button',{name:'Save sharing rules'}));
 await user.click(await screen.findByRole('button',{name:'Confirm and save'}));
 const saving=screen.getByRole('button',{name:'Saving…'});expect(saving).toBeDisabled();
 await user.click(saving);expect(f.api.savePolicy).toHaveBeenCalledTimes(1);
 const pause=screen.getByRole('button',{name:'Pause sharing'});expect(pause).toBeEnabled();await user.click(pause);
 expect(f.api.action).toHaveBeenCalledWith('pause');
 reject(new Error('Settings changed; review again'));
 expect(await screen.findByRole('alert')).toHaveTextContent('Settings changed');
});

it('reviews a 30 to 100 GiB change as total VM space on the picked drive, including its system disk',async()=>{
 const f=fixture();
 const saved={id:'one',volumeId:'data',directory:'/data/NodeHarbor',allocationGib:30,available:true,reason:''};
 f.setSnapshot({...f.snapshot(),resources:{cpus:8,memoryMib:16384,diskGib:25},worker:{installed:true,running:false},storage:{...f.snapshot().storage!,locations:[saved]}});
 Object.assign(f.plan,{requiresRestart:true,layout:{version:1,systemLocationId:'one',volumeId:'data',runtimeDirectory:'/data/NodeHarbor/.nh12345678',systemGib:16}});
 const user=userEvent.setup();render(<App backend={f.api}/>);
 await user.click(await screen.findByRole('button',{name:'Sharing rules'}));
 const allocation=screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'});
 expect(allocation).toHaveValue(30);await user.clear(allocation);await user.type(allocation,'100');
 await user.click(screen.getByRole('button',{name:'Save sharing rules'}));
 const review=await screen.findByRole('alertdialog');
 expect(review).toHaveTextContent('100 GiB total VM storage');
 expect(review).toHaveTextContent('16 GiB system');
 expect(review).toHaveTextContent('84 GiB workload');
 expect(review).toHaveTextContent('/data/NodeHarbor/.nh12345678');
 expect(review).not.toHaveTextContent('Separate system disk');
 await user.click(within(review).getByRole('button',{name:'Confirm and save'}));
 expect(await screen.findByText(/saved.*storage update pending/i)).toBeVisible();
 expect(f.api.action).not.toHaveBeenCalled();
});
