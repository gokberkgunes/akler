
const wasmerTimerSetTimeout = globalThis.setTimeout.bind(globalThis);
const wasmerTimerClearTimeout = globalThis.clearTimeout.bind(globalThis);
const WasmerTimerPromise = globalThis.Promise;
export function wasmer_sdk_start_timer(milliseconds) {
  let finish;
  const promise = new WasmerTimerPromise(resolve => { finish = resolve; });
  const handle = wasmerTimerSetTimeout(finish, milliseconds);
  return [promise, () => {
    wasmerTimerClearTimeout(handle);
    // Settle the Promise too: an abandoned JsFuture otherwise keeps its
    // resolve/reject callbacks and Rust waker alive until the timer expires.
    finish();
  }];
}
