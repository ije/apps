import { useEffect, useSyncExternalStore } from 'react';
import { flushSync } from 'react-dom';
import { createRoot } from 'react-dom/client';

import { PreloadHighlighter } from './components/PreloadHighlighter';
import { ReviewUI } from './components/ReviewUI';
import { ScrollbarGutterVariables } from './components/ScrollbarGutterVariables';
import { themeController } from './components/themeController';
import { ThemeProvider } from './components/ThemeProvider';
import { Toaster } from './components/Toaster';
import { WorkerPoolProvider } from './components/WorkerPoolProvider';
import { HomePage } from './home/HomePage';
import { describeDiffTarget } from './lib/describeDiffTarget';
import { navigate, subscribeLocation } from './lib/navigate';
import { resolveDiffshubViewerRoute } from './lib/resolveDiffshubViewerRoute';
import { SITE_NAME } from './lib/site';

function App() {
  const href = useSyncExternalStore(
    subscribeLocation,
    () => location.pathname + location.search
  );
  const url = new URL(href, location.href);
  const route = resolveDiffshubViewerRoute(
    url.pathname.split('/').filter(Boolean).map(decodeURIComponent),
    url.searchParams.get('domain') ?? undefined
  );
  const label = route.kind === 'render' ? describeDiffTarget(route.upstreamPath) : null;

  useEffect(() => {
    if (route.kind === 'redirect' && route.target !== '/') {
      navigate(route.target, true);
    }
    document.title = label == null ? `${SITE_NAME}, from Pierre` : `${label} — ${SITE_NAME}`;
  }, [route.kind, route.kind === 'redirect' ? route.target : null, label]);

  if (route.kind !== 'render') {
    return <HomePage />;
  }
  return (
    <div className="flex h-dvh flex-col gap-2">
      <ReviewUI
        domain={route.domain}
        initialUrl={route.url}
        path={route.upstreamPath}
      />
    </div>
  );
}

// Links are plain anchors: app paths use history, web links open in the browser.
document.addEventListener('click', (event) => {
  const link = (event.target as Element).closest?.('a[href]');
  const href = link?.getAttribute('href');
  if (href == null || event.defaultPrevented) return;
  event.preventDefault();
  if (/^https?:\/\//.test(href)) diffshub.openExternal(href);
  else navigate(href);
});

document.documentElement.classList.add(
  themeController.getState().resolvedColorScheme
);
const root = createRoot(document.getElementById('root')!);
flushSync(() =>
  root.render(
    <>
      <ScrollbarGutterVariables />
      <WorkerPoolProvider>
        <ThemeProvider attribute="class">
          <App />
          <Toaster />
        </ThemeProvider>
      </WorkerPoolProvider>
      <PreloadHighlighter />
    </>
  )
);
