import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { HoverPreviewWindow } from './components/HoverPreviewWindow';
import { QuickPasteWindow } from './components/QuickPasteWindow';
import './style.css';

const route = new URLSearchParams(location.search).get('window');

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {route === 'quick-paste' ? (
      <QuickPasteWindow />
    ) : route === 'hover-preview' ? (
      <HoverPreviewWindow />
    ) : (
      <App />
    )}
  </StrictMode>,
);
