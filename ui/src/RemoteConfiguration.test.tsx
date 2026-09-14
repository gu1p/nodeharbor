import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { App } from './App';
import { defaultPolicy, type Backend, type Snapshot } from './model';

const machine = ():Snapshot => ({deviceId:'worker',name:'Office node',platform:'macos',architecture:'arm64',state:'paused',reason:'Paused',policy:defaultPolicy(),resources:{cpus:8,memoryMib:16384,diskGib:200},worker:{installed:false,running:false},enrolled:true,controllerUrl:'',version:'test',workloads:[]});
const report = (enabled=true) => ({consent:enabled,revision:4,policy:defaultPolicy(),hardware:machine().resources,allocatedResources:null,capabilities:{storage:false,storageReason:'Additional disks require the multiple-disk runtime feature',localApproval:['startAtLogin']},storageInventory:[],receipts:[]});
function api(fleet=false, overrides:Record<string,unknown>={}) {
 return {mode:fleet?'fleet':'desktop',snapshot:vi.fn().mockResolvedValue({...machine(),configuration:report(false)}),savePolicy:vi.fn().mockResolvedValue(machine()),action:vi.fn().mockResolvedValue(machine()),enroll:vi.fn().mockResolvedValue(machine()),fleet:vi.fn().mockResolvedValue([{...machine(),lastSeen:new Date().toISOString(),eligibleCi:false,eligibleServices:false}]),setRemoteConsent:vi.fn().mockImplementation(async enabled=>({...machine(),configuration:report(enabled)})),configuration:vi.fn().mockResolvedValue({online:true,report:report(),requests:[]}),configureDevice:vi.fn().mockResolvedValue({status:'requested',requestId:'request'}),...overrides} as Backend & {setRemoteConsent:ReturnType<typeof vi.fn>;configureDevice:ReturnType<typeof vi.fn>};
}
it('lets the local owner grant and independently revoke consent without saving rules',async()=>{
 const backend=api();const user=userEvent.setup();render(<App backend={backend}/>);
 await user.click(await screen.findByRole('button',{name:'Sharing rules'}));
 const consent=screen.getByRole('checkbox',{name:'Allow fleet administrators to configure this node'});
 expect(consent).not.toBeChecked();
 expect(consent).toHaveAccessibleDescription(/CPU.*memory.*storage.*schedule.*power.*workload/i);
 await user.click(consent);await waitFor(()=>expect(consent).toBeChecked());
 await user.click(consent);await waitFor(()=>expect(consent).not.toBeChecked());
 expect(backend.setRemoteConsent.mock.calls).toEqual([[true],[false]]);
 expect(backend.savePolicy).not.toHaveBeenCalled();
});
it('offers browser configuration with current capacity and confirms interruptions before sending a versioned edit',async()=>{
 const backend=api(true);const user=userEvent.setup();render(<App backend={backend}/>);
 await user.click(await screen.findByRole('button',{name:'Configure Office node'}));
 expect(await screen.findByText(/8 CPU cores/)).toBeVisible();
 expect(screen.getByText(/Additional disks require/)).toBeVisible();
 expect(screen.getByRole('checkbox',{name:'Start at login'})).toBeDisabled();
 const cpu=screen.getByRole('spinbutton',{name:'CPU cores'});await user.clear(cpu);await user.type(cpu,'3');
 await user.click(screen.getByRole('button',{name:'Review configuration'}));
 const dialog=screen.getByRole('alertdialog',{name:'Apply node configuration?'});
 expect(dialog).toHaveTextContent(/drain.*restart/i);
 expect(backend.configureDevice).not.toHaveBeenCalled();
 expect(within(dialog).getByRole('button',{name:'Keep editing'})).toHaveFocus();
 await user.click(within(dialog).getByRole('button',{name:'Apply configuration'}));
 await waitFor(()=>expect(backend.configureDevice).toHaveBeenCalledWith('worker',expect.objectContaining({requestId:expect.any(String),expectedRevision:4,acknowledgeInterruption:true,policy:expect.objectContaining({resources:expect.objectContaining({cpus:3})})})));
 expect(await screen.findByText('Requested',{exact:true,selector:'strong'})).toBeVisible();
});
it.each([{online:true,enabled:false,message:/owner has not allowed/i},{online:false,enabled:true,message:/offline/i}])('blocks browser edits when consent or fresh inventory is absent: %j',async({online,enabled,message})=>{
 const user=userEvent.setup();render(<App backend={api(true,{configuration:vi.fn().mockResolvedValue({online,report:report(enabled),requests:[]})})}/>);
 await user.click(await screen.findByRole('button',{name:'Configure Office node'}));
 expect(await screen.findByText(message)).toBeVisible();
 expect(screen.getByRole('button',{name:'Review configuration'})).toBeDisabled();
});
it('retains browser edits after a conflict and provides explicit reload',async()=>{
 const user=userEvent.setup();render(<App backend={api(true,{configureDevice:vi.fn().mockRejectedValue(new Error('Settings changed locally; reload current settings'))})}/>);
 await user.click(await screen.findByRole('button',{name:'Configure Office node'}));
 const cpu=await screen.findByRole('spinbutton',{name:'CPU cores'});await user.clear(cpu);await user.type(cpu,'3');
 await user.click(screen.getByRole('button',{name:'Review configuration'}));
 await user.click(screen.getByRole('button',{name:'Apply configuration'}));
 expect(await screen.findByRole('alert')).toHaveTextContent('Settings changed locally');
 expect(cpu).toHaveValue(3);
 expect(screen.getByRole('button',{name:'Reload current settings'})).toBeEnabled();
});
it('replaces the request notice with the node acknowledgment and a readable settings audit',async()=>{
 const backend=api(true);const user=userEvent.setup();const mounted=render(<App backend={backend}/>);
 await user.click(await screen.findByRole('button',{name:'Configure Office node'}));
 await screen.findByRole('spinbutton',{name:'CPU cores'});
 await user.click(screen.getByRole('button',{name:'Review configuration'}));
 await user.click(screen.getByRole('button',{name:'Apply configuration'}));
 await screen.findByText('Requested',{exact:true,selector:'strong'});
 const changed={...defaultPolicy(),idleOnly:true};
 mounted.rerender(<App backend={{...backend,configuration:vi.fn().mockResolvedValue({online:true,report:{...report(),revision:5,policy:changed},requests:[{requestId:'request',status:'applied',actor:'admin@example.test',requestedAt:'2026-09-13T12:00:00Z',beforePolicy:defaultPolicy(),policy:changed,effectivePolicy:changed}]})}}/>);
 expect(await screen.findByText('Applied',{exact:true})).toBeVisible();
 expect(screen.queryByText(/Waiting for the node acknowledgment/)).not.toBeInTheDocument();
 await user.click(screen.getByText('Requested settings and effective values'));
 expect(screen.getByRole('table',{name:'Settings changed by admin@example.test'})).toHaveTextContent('Only while idle');
});
it('keeps disk changes local when the runtime cannot verify its physical storage location',async()=>{
 const user=userEvent.setup();render(<App backend={api(true,{configuration:vi.fn().mockResolvedValue({online:true,report:{...report(),capabilities:{...report().capabilities,diskGrowth:false,diskGrowthReason:'Disk growth requires local approval because the runtime does not report its storage location'}},requests:[]})})}/>);
 await user.click(await screen.findByRole('button',{name:'Configure Office node'}));
 expect(await screen.findByRole('spinbutton',{name:'Disk (GiB)'})).toBeDisabled();
 expect(screen.getByText(/Disk growth requires local approval/)).toBeVisible();
 expect(screen.getByRole('spinbutton',{name:'CPU cores'})).toBeEnabled();
});
it('shows the resolved worker disk location and each volume allocation with free capacity',async()=>{
 const user=userEvent.setup();render(<App backend={api(true,{configuration:vi.fn().mockResolvedValue({online:true,report:{...report(),workerDiskLocation:'/managed/lima/worker/diffdisk',storageInventory:[{label:'Data',mountPoint:'/managed',filesystem:'apfs',availableGib:200,configuredGib:30}]},requests:[]})})}/>);
 await user.click(await screen.findByRole('button',{name:'Configure Office node'}));
 expect(await screen.findByText(/Worker disk location: \/managed\/lima\/worker\/diffdisk/)).toBeVisible();
 expect(screen.getByText(/200 GiB available.*30 GiB configured/)).toBeVisible();
});

const storageInventory=()=>({defaultDirectory:'/worker/storage',supported:true,reason:'',volumes:[{id:'data',label:'Data',mountPoint:'/data',filesystem:'apfs',availableGib:200,configuredGib:30,eligible:true,reason:''}],locations:[{id:'disk1',volumeId:'data',directory:'/data/worker',allocationGib:30,available:true,reason:''}],revision:2,recoveryEnabled:false,activeGib:30,configuredGib:30});
it('reviews additional file-backed disks on the node before sending their exact plan for application',async()=>{
 const storage=storageInventory();const plan={revision:2,locations:[...storage.locations,{id:'disk2',volumeId:'data',directory:'/data/extra',allocationGib:30}],totalGib:60,requiresRestart:true};
 let requests:unknown[]=[];
 const backend=api(true,{configuration:vi.fn().mockImplementation(async()=>({online:true,report:{...report(),storage,capabilities:{...report().capabilities,storage:true}},requests})),configureDevice:vi.fn().mockImplementation(async(_id,edit)=>{const response={...edit,status:edit.operation.type==='storagePreview'?'applied':'requested',result:edit.operation.type==='storagePreview'?plan:null};requests=[response];return response;})});
 const user=userEvent.setup();render(<App backend={backend}/>);await user.click(await screen.findByRole('button',{name:'Configure Office node'}));
 await user.click(await screen.findByRole('button',{name:'Add storage location'}));await user.type(screen.getByRole('textbox',{name:'Directory for disk 2'}),'/data/extra');
 await user.click(screen.getByRole('button',{name:'Review storage changes'}));
 const review=await screen.findByRole('region',{name:'Storage change review'});expect(review).toHaveTextContent(/drains work and restarts/);
 expect(backend.configureDevice).toHaveBeenCalledWith('worker',expect.objectContaining({expectedRevision:4,operation:{type:'storagePreview',selections:expect.arrayContaining([expect.objectContaining({directory:'/data/extra',allocationGib:30})]),options:{}}}));
 expect(backend.configureDevice).toHaveBeenCalledTimes(1);
 await user.click(within(review).getByRole('button',{name:'Apply storage changes'}));
 await waitFor(()=>expect(backend.configureDevice).toHaveBeenLastCalledWith('worker',expect.objectContaining({expectedRevision:4,acknowledgeInterruption:true,operation:{type:'storageApply',plan}})));
 expect(await screen.findByText('Requested',{exact:true,selector:'strong'})).toBeVisible();
});
it('exposes recovery, backup folders, shrink and explicit deletion confirmation remotely',async()=>{
 const storage=storageInventory();const plan={revision:2,locations:[],totalGib:0,requiresRestart:true,maintenance:{kind:'deleteAll',backup:null,minimumGib:0,temporaryBytes:0,deletions:['/data/worker'],downtime:'All worker data is permanently deleted.'}};
 const backend=api(true,{configuration:vi.fn().mockResolvedValue({online:true,report:{...report(),storage,capabilities:{...report().capabilities,storage:true}},requests:[]}),configureDevice:vi.fn().mockImplementation(async(_id,edit)=>({...edit,status:'applied',result:plan}))});
 const user=userEvent.setup();render(<App backend={backend}/>);await user.click(await screen.findByRole('button',{name:'Configure Office node'}));
 expect(await screen.findByRole('checkbox',{name:'Automatically recover after a missing disk'})).toBeEnabled();expect(screen.getByRole('textbox',{name:'Temporary backup folder (optional)'})).toBeEnabled();
 await user.click(screen.getByRole('button',{name:'Shrink allocation for disk 1'}));expect(screen.getByRole('spinbutton',{name:'Allocation for disk 1 (GiB)'})).toHaveFocus();
 await user.click(screen.getByRole('button',{name:'Delete all worker storage'}));
 const confirm=await screen.findByRole('button',{name:'Confirm deletion'});expect(confirm).toBeDisabled();
 await user.click(screen.getByRole('checkbox',{name:'I confirm: delete all worker data'}));expect(confirm).toBeEnabled();
 await user.click(screen.getByRole('button',{name:'Cancel storage changes'}));expect(backend.configureDevice).toHaveBeenCalledTimes(1);
});

it.each([false,true])('merges storage-only local updates without blessing an unrelated remote edit: remote edit=%s',async unrelated=>{
 const initial={...machine(),storage:storageInventory(),configuration:report(true)};
 const changedPolicy={...initial.policy,allowCi:!unrelated,resources:{...initial.policy.resources,diskGib:45}};
 const changed={...initial,policy:changedPolicy,storage:{...storageInventory(),revision:3,locations:[{...storageInventory().locations[0],allocationGib:45}]},configuration:{...report(true),revision:5,policy:changedPolicy}};
 const backend=api(false,{snapshot:vi.fn().mockResolvedValue(initial),previewStorage:vi.fn().mockResolvedValue({revision:2,locations:changed.storage.locations,totalGib:45,requiresRestart:false}),applyStorage:vi.fn().mockResolvedValue(changed)});
 const user=userEvent.setup();render(<App backend={backend}/>);await user.click(await screen.findByRole('button',{name:'Sharing rules'}));
 const cpu=screen.getByRole('spinbutton',{name:'CPU cores'});await user.clear(cpu);await user.type(cpu,'3');
 await user.click(screen.getByRole('button',{name:'Review storage changes'}));await user.click(await screen.findByRole('button',{name:'Apply storage changes'}));
 await user.click(screen.getByRole('button',{name:'Save sharing rules'}));
 await waitFor(()=>expect(backend.savePolicy).toHaveBeenCalledWith(expect.objectContaining({allowCi:true,resources:{cpus:3,memoryMib:4096,diskGib:45}}),unrelated?4:5));
});
it('distinguishes nodes without remote configuration support from nodes awaiting owner consent',async()=>{
 const user=userEvent.setup();render(<App backend={api(true,{configuration:vi.fn().mockResolvedValue({online:true,report:null,requests:[]})})}/>);
 await user.click(await screen.findByRole('button',{name:'Configure Office node'}));
 expect(await screen.findByText(/has not reported remote configuration support/i)).toBeVisible();
 expect(screen.getByRole('button',{name:'Review configuration'})).toBeDisabled();
});

it('labels completed read-only storage reviews separately from applied settings',async()=>{
 const user=userEvent.setup();render(<App backend={api(true,{configuration:vi.fn().mockResolvedValue({online:true,report:report(),requests:[{requestId:'review',status:'applied',operation:{type:'storagePreview',selections:[],options:{}},actor:'admin@example.test'}]})})}/>);
 await user.click(await screen.findByRole('button',{name:'Configure Office node'}));
 expect(await screen.findByText('Reviewed',{exact:true})).toBeVisible();
 expect(screen.queryByText('Applied',{exact:true})).not.toBeInTheDocument();
 await user.click(screen.getByText('Storage review and effective values'));
 expect(screen.getByText(/no settings were changed/i)).toBeVisible();
});

it('shows previous, requested and acknowledged storage choices with the administrator audit',async()=>{
 const before=storageInventory();const after={...before,configuredGib:45,activeGib:45,locations:[{...before.locations[0],allocationGib:45}]};
 const request={requestId:'disks',status:'applied',actor:'admin@example.test',requestedAt:'2026-09-14T00:00:00Z',operation:{type:'storageApply',plan:{revision:2,locations:after.locations,totalGib:45,requiresRestart:true}},beforeStorage:before,effectiveStorage:after};
 const user=userEvent.setup();render(<App backend={api(true,{configuration:vi.fn().mockResolvedValue({online:true,report:report(),requests:[request]})})}/>);
 await user.click(await screen.findByRole('button',{name:'Configure Office node'}));await user.click(await screen.findByText('Storage change and effective values'));
 const table=screen.getByRole('table',{name:'Storage changed by admin@example.test'});
 expect(table).toHaveTextContent(/Previous.*Requested.*Node acknowledgment/);expect(table).toHaveTextContent('30 GiB');expect(table).toHaveTextContent('45 GiB');
});
