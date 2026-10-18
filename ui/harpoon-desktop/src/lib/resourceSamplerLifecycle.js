export function subscribeResourceSampler({ listen, start, stop, onSnapshot }) {
  let disposed = false;
  let unlisten;
  const ready = listen((payload) => { if (!disposed) onSnapshot(payload); });
  Promise.resolve(ready).then((cleanup) => {
    if (disposed) cleanup?.();
    else unlisten = cleanup;
  });
  start();
  return () => {
    disposed = true;
    unlisten?.();
    stop();
  };
}
