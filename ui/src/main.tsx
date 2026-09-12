import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App';
import { backend } from './backend';
import './styles.css';
ReactDOM.createRoot(document.getElementById('root')!).render(<React.StrictMode><App backend={backend}/></React.StrictMode>);
