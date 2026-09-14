import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { App } from './App';
import { defaultPolicy, type Backend, type Snapshot, type Workload } from './model';

const probe: Workload = {name:'nodeharbor-probe-abc12',namespace:'nodeharbor-system',state:'running'};
const job: Workload = {name:'build-customer-app',namespace:'nodeharbor-ci',state:'running'};
function snapshot(workloads: Workload[]): Snapshot {
 return {deviceId:'worker',name:'My computer',platform:'macos',architecture:'arm64',state:'sharing',reason:'Available for work',policy:{...defaultPolicy(),enabled:true},resources:{cpus:8,memoryMib:16384,diskGib:100},worker:{installed:true,running:true},enrolled:true,controllerUrl:'',version:'test',workloads};
}
function backend(workloads: Workload[]): Backend {
 return {snapshot:vi.fn().mockResolvedValue(snapshot(workloads)),savePolicy:vi.fn(),action:vi.fn(),enroll:vi.fn(),fleet:vi.fn().mockResolvedValue([])};
}

describe('Understanding the work on my computer',()=>{
 it('shows a lone health check as a system component and reports no actual workloads',async()=>{
  const user=userEvent.setup();render(<App backend={backend([probe])}/>);
  const workloads=within(await screen.findByRole('region',{name:'Running workloads'}));
  expect(workloads.getByText('0 active')).toBeVisible();
  expect(workloads.getByRole('heading',{name:'No workloads running'})).toBeVisible();
  expect(workloads.queryByText('NodeHarbor health check')).not.toBeInTheDocument();
  const systems=within(screen.getByRole('region',{name:'System components'}));
  expect(systems.getByText('1 running')).toBeVisible();
  expect(systems.getByText('NodeHarbor health check')).toBeVisible();
  expect(systems.getByText('Checks your worker’s connection to the cluster.')).toBeVisible();
  expect(systems.getByText(probe.name)).not.toBeVisible();
  await user.click(systems.getByText('Component details'));
  expect(systems.getByText(probe.name)).toBeVisible();
  expect(systems.getByText(probe.namespace)).toBeVisible();
  expect(screen.getByRole('button',{name:'Pause sharing'})).toBeEnabled();
 });

 it('counts real jobs separately and keeps unfamiliar system components visible',async()=>{
  const other={name:'worker-helper',namespace:'nodeharbor-system',state:'running'};
  render(<App backend={backend([probe,job,other])}/>);
  const workloads=within(await screen.findByRole('region',{name:'Running workloads'}));
  expect(workloads.getByText('1 active')).toBeVisible();
  expect(workloads.getByText(job.name)).toBeVisible();
  const systems=within(screen.getByRole('region',{name:'System components'}));
  expect(systems.getByText('2 running')).toBeVisible();
  expect(systems.getByText(other.name)).toBeVisible();
  expect(systems.queryByText(job.name)).not.toBeInTheDocument();
 });

 it('keeps both sections understandable when the worker has no running containers',async()=>{
  render(<App backend={backend([])}/>);
  const workloads=within(await screen.findByRole('region',{name:'Running workloads'}));
  expect(workloads.getByText('0 active')).toBeVisible();
  expect(workloads.getByRole('heading',{name:'No workloads running'})).toBeVisible();
  const systems=within(screen.getByRole('region',{name:'System components'}));
  expect(systems.getByText('0 running')).toBeVisible();
  expect(systems.getByText('No system components running')).toBeVisible();
 });

 it('updates job counts after refresh, preserves the health check, and recovers from a refresh error',async()=>{
  const api=backend([probe]);
  api.snapshot=vi.fn().mockResolvedValueOnce(snapshot([probe]))
   .mockResolvedValueOnce(snapshot([probe,job]))
   .mockRejectedValueOnce(new Error('Worker status is unavailable'))
   .mockResolvedValue(snapshot([probe]));
  const user=userEvent.setup();render(<App backend={api}/>);
  expect(screen.getByRole('status')).toHaveTextContent('Loading your workspace');
  const workloads=within(await screen.findByRole('region',{name:'Running workloads'}));
  expect(workloads.getByText('0 active')).toBeVisible();
  await user.click(screen.getByRole('button',{name:'Refresh machine status'}));
  expect(await workloads.findByText(job.name)).toBeVisible();
  expect(workloads.getByText('1 active')).toBeVisible();
  await user.click(screen.getByRole('button',{name:'Refresh machine status'}));
  expect(await screen.findByRole('alert')).toHaveTextContent('Worker status is unavailable');
  expect(workloads.getByText(job.name)).toBeVisible();
  expect(within(screen.getByRole('region',{name:'System components'})).getByText('NodeHarbor health check')).toBeVisible();
  await user.click(screen.getByRole('button',{name:'Try again'}));
  expect(await workloads.findByText('0 active')).toBeVisible();
  expect(workloads.queryByText(job.name)).not.toBeInTheDocument();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(api.action).not.toHaveBeenCalled();
 });
});
