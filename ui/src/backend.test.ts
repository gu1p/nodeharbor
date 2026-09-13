import { afterEach, describe, expect, it, vi } from 'vitest';
import { defaultPolicy } from './model';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';

afterEach(() => { clearMocks(); vi.resetModules(); });
describe('The native desktop bridge', () => {
  it('routes worker controls through native commands without sending credentials to the UI', async () => {
    const commands: { cmd: string; payload: unknown }[] = [];
    mockIPC((cmd, payload) => { commands.push({ cmd, payload }); return { state: 'paused' }; });
    const { backend } = await import('./backend');
    expect(backend.mode).toBe('desktop');
    await backend.snapshot();
    await (backend as typeof backend & {activity:()=>Promise<unknown>}).activity();
    await backend.action('pause');
    await backend.enroll('https://workers.example.com', 'one-use-code');
    expect(commands).toEqual([
      { cmd: 'snapshot', payload: {} },
      { cmd: 'activity', payload: {} },
      { cmd: 'worker_action', payload: { action: 'pause' } },
      { cmd: 'enroll', payload: { url: 'https://workers.example.com', code: 'one-use-code' } },
    ]);
  });
});

it('passes explicitly confirmed replacement through the native worker command',async()=>{
 const commands: {cmd:string;payload:unknown}[]=[];
 mockIPC((cmd,payload)=>{commands.push({cmd,payload}); return {state:'paused'};});
 const {backend}=await import('./backend');
 await (backend as typeof backend & {recreateWorker:(policy:ReturnType<typeof defaultPolicy>)=>Promise<unknown>}).recreateWorker(defaultPolicy());
 expect(commands).toEqual([{cmd:'recreate_worker',payload:{policy:defaultPolicy()}}]);
});
