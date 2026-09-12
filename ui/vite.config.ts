import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
export default defineConfig({define:{__APP_VERSION__:JSON.stringify(process.env.NODEHARBOR_VERSION ?? "0.1.0")},plugins:[react()],server:{port:1420,strictPort:true},test:{environment:'jsdom',setupFiles:['./src/test-setup.ts'],restoreMocks:true}});
