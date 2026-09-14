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
 expect(await screen.findByText('Requested',{exact:true})).toBeVisible();
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
 await screen.findByText('Requested',{exact:true});
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
