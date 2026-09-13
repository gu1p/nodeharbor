import {render,screen,within,waitFor} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {describe,it,expect,vi} from 'vitest';
import {App} from './App';
import {defaultPolicy,type Backend,type Snapshot,type UpdateStatus} from './model';
const machine:Snapshot={deviceId:'worker',name:'My computer',platform:'macos',architecture:'arm64',state:'sharing',reason:'Available',policy:{...defaultPolicy(),enabled:true},resources:{cpus:8,memoryMib:16000,diskGib:100},worker:{installed:true,running:true},enrolled:true,controllerUrl:'',version:'0.1.67',workloads:[]};
function fixture(phase='idle',message='You are up to date') {
 const status:UpdateStatus={enabled:true,phase,message,currentVersion:'0.1.67',availableVersion:null,downloaded:0,total:null};
 return {snapshot:vi.fn().mockResolvedValue(machine),savePolicy:vi.fn(),action:vi.fn(),enroll:vi.fn(),fleet:vi.fn(),updates:vi.fn().mockResolvedValue(status),updateAction:vi.fn().mockImplementation(async(action:string)=>({...status,enabled:action==='disable'?false:true}))};
}
describe('Keeping NodeHarbor up to date',()=>{
 it('makes automatic updates and a manual check reachable without changing sharing rules',async()=>{
  const api=fixture();const user=userEvent.setup();render(<App backend={api as Backend}/>);
  await user.click(await screen.findByRole('button',{name:'App updates'}));
  const panel=within(await screen.findByRole('region',{name:'App updates'}));
  expect(panel.getByRole('checkbox',{name:'Automatically install updates'})).toBeChecked();
  expect(panel.getByText('You are up to date')).toBeVisible();
  await user.click(panel.getByRole('button',{name:'Check for updates'}));
  expect(api.updateAction).toHaveBeenCalledWith('check');
  await user.click(panel.getByRole('checkbox',{name:'Automatically install updates'}));
  await waitFor(()=>expect(panel.getByRole('checkbox')).not.toBeChecked());
  expect(api.updateAction).toHaveBeenCalledWith('disable');
  expect(api.action).not.toHaveBeenCalled();expect(api.savePolicy).not.toHaveBeenCalled();
 });
 it('explains why installation waits for a job and allows cancellation',async()=>{
  const api=fixture('waiting','Waiting for running jobs to finish. New assignments are paused.');
  const user=userEvent.setup();render(<App backend={api as Backend}/>);
  await user.click(await screen.findByRole('button',{name:'App updates'}));
  expect(await screen.findByText(/Waiting for running jobs to finish/)).toBeVisible();
  await user.click(screen.getByRole('button',{name:'Cancel update'}));
  expect(api.updateAction).toHaveBeenCalledWith('cancel');
 });
 it('shows download progress, loading, and a recoverable check error',async()=>{
  const api=fixture();api.updates.mockRejectedValueOnce(new Error('Update service is unavailable'));
  const user=userEvent.setup();render(<App backend={api as Backend}/>);
  await user.click(await screen.findByRole('button',{name:'App updates'}));
  expect(await screen.findByRole('alert')).toHaveTextContent('Update service is unavailable');
  api.updates.mockResolvedValue({...await fixture().updates(),phase:'downloading',message:'Downloading update',downloaded:50,total:100});
  await user.click(screen.getByRole('button',{name:'Retry update status'}));
  expect(await screen.findByRole('progressbar',{name:'Update download'})).toHaveAttribute('value','50');
  expect(screen.getByRole('button',{name:'Check for updates'})).toBeDisabled();
 });
 it('provides an explicit install action when automatic installation is off',async()=>{
  const api=fixture('available','NodeHarbor 0.1.68 is available');
  api.updates.mockResolvedValue({...await api.updates(),enabled:false,availableVersion:'0.1.68'});
  const user=userEvent.setup();render(<App backend={api as Backend}/>);
  await user.click(await screen.findByRole('button',{name:'App updates'}));
  await user.click(await screen.findByRole('button',{name:'Install update'}));
  expect(api.updateAction).toHaveBeenCalledWith('install');
 });
});

it('keeps update controls disabled while cancellation releases worker maintenance',async()=>{
 const api=fixture('cancelling','Cancelling the update…');const user=userEvent.setup();
 render(<App backend={api as Backend}/>);
 await user.click(await screen.findByRole('button',{name:'App updates'}));
 expect(await screen.findByText('Cancelling the update…')).toBeVisible();
 expect(screen.getByRole('button',{name:'Check for updates'})).toBeDisabled();
 expect(screen.queryByRole('button',{name:'Install update'})).not.toBeInTheDocument();
});
