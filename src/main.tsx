import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { HoverPreviewWindow } from './components/HoverPreviewWindow';
import { QuickPasteWindow } from './components/QuickPasteWindow';
import { TrayMenuWindow } from './components/TrayMenuWindow';
import './style.css';

const route = new URLSearchParams(location.search).get('window');
if (route === 'tray-menu') document.body.classList.add('tray-menu-window');

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {route === 'quick-paste' ? (
      <QuickPasteWindow />
    ) : route === 'hover-preview' ? (
      <HoverPreviewWindow />
    ) : route === 'tray-menu' ? (
      <TrayMenuWindow />
    ) : (
      <App />
    )}
  </StrictMode>,
);
