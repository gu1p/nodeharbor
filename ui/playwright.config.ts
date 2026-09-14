import { defineConfig } from '@playwright/test';
export default defineConfig({testDir:'./tests',testMatch:'**/*.e2e.ts',projects:[{name:'chromium',use:{browserName:'chromium'}},{name:'webkit',use:{browserName:'webkit'}}],use:{baseURL:'http://127.0.0.1:1421'},webServer:{command:'npm run dev -- --port 1421',url:'http://127.0.0.1:1421',reuseExistingServer:false}});
