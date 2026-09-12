import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, it, expect, vi } from 'vitest';
import { App } from './App';
import { defaultPolicy, type Backend, type Snapshot } from './model';
const snapshot = (): Snapshot => ({deviceId:'pilot',name:'My Mac',platform:'macos',architecture:'arm64',state:'paused',reason:'Sharing is switched off',policy:defaultPolicy(),resources:{cpus:8,memoryMib:16384,diskGib:100},worker:{installed:false,running:false},enrolled:false,controllerUrl:'',version:'0.1.0',workloads:[]});
function backend(overrides: Partial<Backend> = {}): Backend {
  return {snapshot:vi.fn().mockResolvedValue(snapshot()),savePolicy:vi.fn().mockImplementation(async policy => ({...snapshot(),policy})),action:vi.fn().mockResolvedValue(snapshot()),enroll:vi.fn().mockResolvedValue(snapshot()),fleet:vi.fn().mockResolvedValue([]),...overrides};
}
describe('A person contributes a machine', () => {
 it('shows loading, then the sharing state and all core navigation', async () => {
  render(<App backend={backend()}/>);
  expect(screen.getByRole('status')).toHaveTextContent('Loading');
  expect(await screen.findByRole('heading',{name:'Your machine'})).toBeVisible();
  for (const name of ['Your machine','Sharing rules','Fleet','Connection']) expect(screen.getByRole('button',{name})).toBeEnabled();
  expect(screen.getByText('Sharing is switched off')).toBeVisible();
 });
 it('lets a person change RAM and CPU and saves every opt-in control', async () => {
  const api=backend(); const user=userEvent.setup(); render(<App backend={api}/>);
  await user.click(await screen.findByRole('button',{name:'Sharing rules'}));
  const cpu=screen.getByRole('spinbutton',{name:'CPU cores'}); await user.clear(cpu); await user.type(cpu,'3');
  const ram=screen.getByRole('spinbutton',{name:'RAM (GiB)'}); await user.clear(ram); await user.type(ram,'6');
  for(const name of ['Only while idle','Allow sharing on battery','Use a weekly schedule','Start at login','Continue when the window closes','Accept CI jobs','Accept eligible services']) expect(screen.getByRole('checkbox',{name})).toBeEnabled();
  await user.click(screen.getByRole('checkbox',{name:'Only while idle'}));
  await user.click(screen.getByRole('button',{name:'Save sharing rules'}));
  await waitFor(()=>expect(api.savePolicy).toHaveBeenCalledWith(expect.objectContaining({idleOnly:true,resources:expect.objectContaining({cpus:3,memoryMib:6144})})));
 });
 it('keeps a failed save visible and preserves the edited value', async () => {
  const user=userEvent.setup(); render(<App backend={backend({savePolicy:vi.fn().mockRejectedValue(new Error('Not enough available memory'))})}/>);
  await user.click(await screen.findByRole('button',{name:'Sharing rules'}));
  await user.click(screen.getByRole('button',{name:'Save sharing rules'}));
  expect(await screen.findByRole('alert')).toHaveTextContent('Not enough available memory');
  expect(screen.getByRole('button',{name:'Save sharing rules'})).toBeEnabled();
 });
 it('shows an empty fleet and offers a way to connect a machine', async () => {
  const user=userEvent.setup(); render(<App backend={backend()}/>);
  await user.click(await screen.findByRole('button',{name:'Fleet'}));
  expect(await screen.findByText('No machines connected yet')).toBeVisible();
  await user.click(screen.getByRole('button',{name:'Connect this machine'}));
  expect(screen.getByRole('textbox',{name:'Controller URL'})).toBeVisible();
 });
 it('makes connection errors recoverable', async () => {
  const api=backend({snapshot:vi.fn().mockRejectedValueOnce(new Error('Agent unavailable')).mockResolvedValue(snapshot())});
  render(<App backend={api}/>); const user=userEvent.setup();
  expect(await screen.findByRole('alert')).toHaveTextContent('Agent unavailable');
  await user.click(screen.getByRole('button',{name:'Try again'}));
  expect(await screen.findByRole('heading',{name:'Your machine'})).toBeVisible();
 });
 it('requires an explicit choice before immediately interrupting workloads', async () => {
  const api=backend({snapshot:vi.fn().mockResolvedValue({...snapshot(),enrolled:true,state:'sharing',worker:{installed:true,running:true}})});
  const user=userEvent.setup(); render(<App backend={api}/>);
  await user.click(await screen.findByRole('button',{name:'Stop now'}));
  expect(screen.getByRole('dialog',{name:'Stop running work?'})).toBeVisible();
  expect(api.action).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button',{name:'Stop and interrupt work'}));
  await waitFor(()=>expect(api.action).toHaveBeenCalledWith('stop'));
 });
});
