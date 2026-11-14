import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import { HoverPreviewWindow } from './components/HoverPreviewWindow';
import './style.css';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {new URLSearchParams(location.search).get('window') === 'hover-preview' ? (
      <HoverPreviewWindow />
    ) : (
      <App />
    )}
  </StrictMode>,
);
