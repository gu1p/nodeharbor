import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ActivityPanel } from './ActivityPanel';

const entry = (id:number, message:string, level='info') => ({id, timestamp:'2026-09-13T12:00:00Z', source:'worker', level, message});
const activity = (entries = [entry(1,'Waiting for the VM to receive an IP address')]) => ({entries, dropped:0, step:{message:'Creating Ubuntu VM',startedAt:'2026-09-13T12:00:00Z',timeoutSeconds:1200}});
afterEach(()=>vi.useRealTimers());

it('keeps the activity panel accessible during loading and the empty state', async()=>{
 let finish!:(value:unknown)=>void;
 const load=vi.fn(()=>new Promise(resolve=>{finish=resolve;}));
 render(<ActivityPanel load={load as never}/>);
 expect(screen.getByRole('heading',{name:'Worker activity'})).toBeVisible();
 expect(screen.getByText('Loading activity…')).toBeVisible();
 expect(screen.getByRole('checkbox',{name:'Follow latest output'})).toBeChecked();
 expect(screen.getByRole('button',{name:'Copy logs'})).toBeDisabled();
 await act(async()=>finish({...activity([]),step:null}));
 expect(screen.getByText('Worker activity will appear here when you prepare or start sharing.')).toBeVisible();
 expect(screen.getByRole('log',{name:'Worker activity log'})).toBeVisible();
});

it('shows new output within a second while preparation is running, with elapsed time', async()=>{
 vi.useFakeTimers();vi.setSystemTime(new Date('2026-09-13T12:00:45Z'));
 const load=vi.fn().mockResolvedValueOnce(activity()).mockResolvedValue(activity([entry(1,'Waiting for the VM to receive an IP address'),entry(2,'Downloading NetBird: 25%')]));
 render(<ActivityPanel load={load}/>);
 await act(async()=>{});
 expect(screen.getByText('Creating Ubuntu VM')).toBeVisible();
 expect(screen.getByText(/45s elapsed/)).toBeVisible();
 expect(screen.getByText(/No new output for 45s/)).toBeVisible();
 await act(async()=>{vi.advanceTimersByTime(1000);});
 expect(screen.getByRole('log')).toHaveTextContent('Downloading NetBird: 25%');
 expect(screen.getByText(/46s elapsed/)).toBeVisible();
});

it('retains diagnostics after a failed refresh and offers a working reconnect action', async()=>{
 vi.useFakeTimers();
 const load=vi.fn().mockResolvedValueOnce(activity([entry(1,'VM launch timed out','error')])).mockRejectedValueOnce(new Error('Agent disconnected')).mockResolvedValue(activity([entry(2,'Preparation requested again')]));
 render(<ActivityPanel load={load}/>);
 await act(async()=>{});
 await act(async()=>{vi.advanceTimersByTime(1000);});
 expect(screen.getByRole('log')).toHaveTextContent('VM launch timed out');
 expect(screen.getByRole('alert')).toHaveTextContent('Agent disconnected');
 await act(async()=>{fireEvent.click(screen.getByRole('button',{name:'Reconnect logs'}));});
 expect(screen.getByRole('log')).toHaveTextContent('Preparation requested again');
 expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});

it('does not overlap slow requests or keep polling after the panel closes',async()=>{
 vi.useFakeTimers();
 const load=vi.fn(()=>new Promise(()=>{}));
 const view=render(<ActivityPanel load={load as never}/>);
 await act(async()=>{vi.advanceTimersByTime(5000);});
 expect(load).toHaveBeenCalledTimes(1);
 view.unmount();
 await act(async()=>{vi.advanceTimersByTime(5000);});
 expect(load).toHaveBeenCalledTimes(1);
});

it('allows reading older output and copying the visible diagnostics',async()=>{
 const user=userEvent.setup();
 const copy=vi.spyOn(navigator.clipboard,'writeText').mockResolvedValue();
 render(<ActivityPanel load={vi.fn().mockResolvedValue({...activity(),dropped:32})}/>);
 await screen.findByText('Waiting for the VM to receive an IP address');
 expect(screen.getByText(/32 older entries/)).toBeVisible();
 await user.click(screen.getByRole('checkbox',{name:'Follow latest output'}));
 expect(screen.getByRole('checkbox',{name:'Follow latest output'})).not.toBeChecked();
 await user.click(screen.getByRole('button',{name:'Copy logs'}));
 await waitFor(()=>expect(copy).toHaveBeenCalledWith(expect.stringContaining('Waiting for the VM to receive an IP address')));
 expect(screen.getByText('Logs copied')).toBeVisible();
});
