import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { App } from './App';
import { defaultPolicy, type Backend, type Policy } from './model';

function fixture(pending = false) {
 const state = {deviceId:'owner',name:'My computer',platform:'macos',architecture:'arm64',state:'paused',reason:'Sharing is switched off',policy:defaultPolicy(),resources:{cpus:8,memoryMib:16384,diskGib:100},allocatedResources:{cpus:2,memoryMib:4096,diskGib:30},recreationPending:pending,worker:{installed:true,running:false},enrolled:true,controllerUrl:'https://workers.example.com',version:'test',workloads:[]};
 const api: Backend & {recreateWorker: ReturnType<typeof vi.fn>} = {snapshot:vi.fn().mockResolvedValue(state),savePolicy:vi.fn().mockResolvedValue(state),action:vi.fn().mockResolvedValue(state),enroll:vi.fn().mockResolvedValue(state),fleet:vi.fn().mockResolvedValue([]),recreateWorker:vi.fn().mockImplementation(async (policy:Policy)=>({...state,policy:{...policy,enabled:false},recreationPending:true}))};
 return {state,api};
}
async function shrink(api:Backend) {
 const user=userEvent.setup(); render(<App backend={api}/>);
 await user.click(await screen.findByRole('button',{name:'Sharing rules'}));
 const disk=screen.getByRole('spinbutton',{name:'Disk (GiB)'});
 await user.clear(disk); await user.type(disk,'20');
 await user.click(screen.getByRole('checkbox',{name:'Only while idle'}));
 await user.click(screen.getByRole('button',{name:'Save sharing rules'}));
 return user;
}
it('explains disk deletion before saving a smaller budget and makes cancellation keyboard accessible',async()=>{
 const {api}=fixture(); const user=await shrink(api);
 const dialog=screen.getByRole('alertdialog',{name:'Replace the worker disk?'});
 expect(dialog).toHaveTextContent('30 GiB'); expect(dialog).toHaveTextContent('20 GiB');
 expect(dialog).toHaveTextContent(/permanently delete/i);
 expect(dialog).toHaveTextContent(/sharing stays off/i);
 expect(dialog).toHaveTextContent(/prepare/i);
 expect(api.savePolicy).not.toHaveBeenCalled(); expect(api.recreateWorker).not.toHaveBeenCalled();
 const cancel=screen.getByRole('button',{name:'Keep current worker'});
 await waitFor(()=>expect(cancel).toHaveFocus());
 await user.tab({shift:true}); expect(screen.getByRole('button',{name:'Delete disk and save rules'})).toHaveFocus();
 await user.tab(); expect(cancel).toHaveFocus();
 await user.keyboard('{Escape}');
 expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
 expect(screen.getByRole('button',{name:'Save sharing rules'})).toHaveFocus();
 expect(screen.getByRole('spinbutton',{name:'Disk (GiB)'})).toHaveValue(20);
 expect(api.recreateWorker).not.toHaveBeenCalled();
});
it('submits all edited rules only after confirmation and exposes pending replacement',async()=>{
 const {api}=fixture(); const user=await shrink(api);
 await user.click(screen.getByRole('button',{name:'Delete disk and save rules'}));
 await waitFor(()=>expect(api.recreateWorker).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({idleOnly:true,resources:expect.objectContaining({diskGib:20})})));
 expect(api.savePolicy).not.toHaveBeenCalled();
 expect(await screen.findByText(/Worker replacement is pending/)).toBeVisible();
 expect(screen.getByRole('button',{name:'Save sharing rules'})).toBeDisabled();
 await user.click(screen.getByRole('button',{name:'Your machine'}));
 expect(screen.queryByRole('button',{name:'Start sharing'})).not.toBeInTheDocument();
 expect(screen.queryByRole('button',{name:'Prepare worker'})).not.toBeInTheDocument();
});
it('keeps replacement errors visible and edited settings available for retry',async()=>{
 const {api}=fixture(); api.recreateWorker.mockRejectedValue(new Error('The worker has no ownership receipt'));
 const user=await shrink(api);
 await user.click(screen.getByRole('button',{name:'Delete disk and save rules'}));
 expect(await screen.findByRole('alert')).toHaveTextContent('no ownership receipt');
 expect(screen.getByRole('spinbutton',{name:'Disk (GiB)'})).toHaveValue(20);
 expect(screen.getByRole('checkbox',{name:'Only while idle'})).toBeChecked();
 expect(screen.getByRole('button',{name:'Save sharing rules'})).toBeEnabled();
});
it('prevents duplicate destructive requests while the desktop command is pending',async()=>{
 const {api}=fixture(); let finish!:(value:unknown)=>void;
 api.recreateWorker.mockReturnValue(new Promise(resolve=>{finish=resolve;}));
 const user=await shrink(api); await user.click(screen.getByRole('button',{name:'Delete disk and save rules'}));
 expect(screen.getByRole('button',{name:'Saving…'})).toBeDisabled();
 expect(api.recreateWorker).toHaveBeenCalledTimes(1);
 await act(async()=>finish(await api.snapshot()));
});
