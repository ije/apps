declare global {
  var diffshub: {
    openExternal(url: string): void;
    request(
      path: string,
      headers: Record<string, string>,
      write: (chunk: string | null, error?: string) => boolean
    ): Promise<{ status: number; headers: [string, string][] }>;
  };
}

// Calls the API routes in the preload script and streams the body like fetch.
export async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const encoder = new TextEncoder();
  let controller!: ReadableStreamDefaultController<Uint8Array>;
  let open = true;
  const body = new ReadableStream<Uint8Array>({
    start: (value) => {
      controller = value;
    },
    cancel: () => {
      open = false;
    },
  });
  init.signal?.addEventListener(
    'abort',
    () => {
      if (open) controller.error(init.signal!.reason);
      open = false;
    },
    { once: true }
  );
  const { status, headers } = await diffshub.request(
    path,
    Object.fromEntries(new Headers(init.headers)),
    (chunk, error) => {
      if (!open) return false;
      if (error != null) controller.error(new Error(error));
      else if (chunk == null) controller.close();
      else return controller.enqueue(encoder.encode(chunk)), true;
      open = false;
      return false;
    }
  );
  return new Response(body, { status, headers });
}
