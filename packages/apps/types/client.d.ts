/// <reference types="node" />

interface ImportMetaEnv {
  readonly MODE: 'development' | 'production'
  readonly DEV: boolean
  readonly PROD: boolean
  readonly SSR: false
  readonly BASE_URL: string
  /** Values of `env` in package.json. */
  readonly [key: string]: any
}

interface ImportMeta {
  readonly env: ImportMetaEnv
  /** Maps each matching file to its URL at build time. */
  glob(pattern: string, options?: { eager?: boolean; query?: string; import?: string }): Record<string, string>
}

declare namespace NodeJS {
  interface ProcessVersions {
    readonly apps: string
    readonly electron: string
  }
  interface Process {
    /** `browser` in main, `renderer` in preload scripts. */
    readonly type: 'browser' | 'renderer' | 'utility'
    /** In a process from `utilityProcess.fork()`. */
    readonly parentPort?: import('node:events').EventEmitter<{ message: [event: { data: any; ports: [] }] }> & { postMessage(message: any): void }
  }
}

// Empty so these resolve without the DOM lib. They merge with the DOM types.
interface Worker {}
interface SharedWorker {}

// Empty so the terminal types resolve without the DOM lib.
interface HTMLElement {}

/** A record of the program status protocol (OSC 7501). The record of the program itself has an empty `id`. */
interface TerminalProgramStatus {
  /** `build/test` is a child of `build`. */
  readonly id: string
  readonly state: 'idle' | 'working' | 'done' | 'blocked' | 'error'
  /** What a blocked program needs. */
  readonly kind: 'permission' | 'question' | 'auth' | null
  /** 0 to 100, or `null` when the program gave no percentage. */
  readonly progress: number | null
  /** A stable name of the program, like `cargo`. A record without one takes it from its parent. */
  readonly app: string
  readonly title: string
  readonly message: string
}

interface TerminalSessionEventMap {
  /** The program ended. */
  exit: Event & { readonly exitCode: number | null; readonly signal: string | null }
  /** Input for the program of a session that has no program: keys, pastes and answers of the terminal. */
  data: Event & { readonly data: string }
  /** `cols` or `rows` changed. */
  resize: Event
  titlechange: Event
  cwdchange: Event
  bell: Event
  /** `programStatus` changed. */
  statuschange: Event
  /** The session ended: `close()` was called, or its element left the document. */
  close: Event
}

/**
 * A terminal with its program. A preload script creates it with `new terminal.Session()` from `@pierre/apps`
 * and gives it to the page, where a `<terminal>` element shows it. The page and the preload script have
 * their own object for the same session.
 */
interface TerminalSession extends EventTarget {
  /** Calls on a closed session throw. */
  readonly closed: boolean
  /** The size of the grid: 80 by 24, and then the size of the element as of its last frame. */
  readonly cols: number
  readonly rows: number
  /** The cursor in the grid. */
  readonly cursor: { readonly x: number; readonly y: number; readonly visible: boolean }
  /** The process ID of the program, or `null` when no program runs. */
  readonly pid: number | null
  /** The process in the foreground, like `vim` in a shell. */
  readonly process: { readonly pid: number; readonly name: string } | null
  /** The directory that the shell reports, or else the directory of the foreground process. */
  readonly cwd: string
  /** The title that the program sets. */
  readonly title: string
  /** The records that programs report with OSC 7501. */
  readonly programStatus: readonly TerminalProgramStatus[]
  /** A full-screen program like `vim` uses the alternate screen. */
  readonly alternateScreen: boolean
  /** All text, with the scrollback. */
  readonly text: string
  readonly selectedText: string
  /**
   * Preload only. Starts a program and ends the one before it. A string is a command line for the login
   * shell, an array is the program and its arguments, and without a command it is the login shell. Returns
   * the process ID.
   */
  spawn(command?: string | string[] | null, options?: { cwd?: string; env?: Record<string, string> }): number
  /** Sends a signal to the program. The default is `SIGHUP`. */
  kill(signal?: 'SIGHUP' | 'SIGINT' | 'SIGQUIT' | 'SIGKILL' | 'SIGTERM'): void
  /** Sends input to the program, as if the user typed it. */
  input(data: string | ArrayBuffer | ArrayBufferView): void
  /** Sends text to the program as a paste. */
  paste(text: string): void
  /** Shows output on the screen, as if the program wrote it. */
  output(data: string | ArrayBuffer | ArrayBufferView): void
  /** Clears the screen and the scrollback. */
  reset(): void
  selectAll(): void
  clearSelection(): void
  scrollToTop(): void
  scrollToBottom(): void
  /** Scrolls down by rows, or up when `count` is negative. */
  scrollLines(count: number): void
  /** Ends the program and the session. */
  close(): void
  addEventListener<K extends keyof TerminalSessionEventMap>(type: K, listener: (this: TerminalSession, event: TerminalSessionEventMap[K]) => any, options?: boolean | AddEventListenerOptions): void
  addEventListener(type: string, listener: ((event: Event) => any) | { handleEvent(event: Event): any }, options?: boolean | AddEventListenerOptions): void
  removeEventListener<K extends keyof TerminalSessionEventMap>(type: K, listener: (this: TerminalSession, event: TerminalSessionEventMap[K]) => any, options?: boolean | EventListenerOptions): void
  removeEventListener(type: string, listener: ((event: Event) => any) | { handleEvent(event: Event): any }, options?: boolean | EventListenerOptions): void
}

/**
 * A `<terminal>` element. It needs the `terminal` feature. A cell is `1ch` wide and `1lh` tall, and the
 * `-terminal-*` properties set the 16 colors.
 */
interface HTMLTerminalElement extends HTMLElement {
  /**
   * The session that the element shows. Without one, the element is an empty box. A session shows in one
   * element, and it ends when its element leaves the document.
   */
  session: TerminalSession | null
}

interface HTMLElementTagNameMap {
  terminal: HTMLTerminalElement
}

declare module '*?raw' {
  const text: string
  export default text
}

declare module '*?url' {
  const url: string
  export default url
}

declare module '*?worker' {
  const WorkerClass: new (options?: { name?: string }) => Worker
  export default WorkerClass
}

declare module '*?sharedworker' {
  const SharedWorkerClass: new (options?: { name?: string }) => SharedWorker
  export default SharedWorkerClass
}

/** In pages, the import also adds the sheet to the page. */
declare module '*.css' {
  const css: string
  export default css
}

declare module '*.html' {
  const html: string
  export default html
}

/** Needs `with { type: 'text' }`. */
declare module '*.txt' {
  const text: string
  export default text
}

declare module '*.node' {
  const addon: any
  export default addon
}

declare module '*.png' { const url: string; export default url }
declare module '*.jpg' { const url: string; export default url }
declare module '*.jpeg' { const url: string; export default url }
declare module '*.gif' { const url: string; export default url }
declare module '*.webp' { const url: string; export default url }
declare module '*.avif' { const url: string; export default url }
declare module '*.bmp' { const url: string; export default url }
declare module '*.ico' { const url: string; export default url }
declare module '*.svg' { const url: string; export default url }
declare module '*.ttf' { const url: string; export default url }
declare module '*.otf' { const url: string; export default url }
declare module '*.woff' { const url: string; export default url }
declare module '*.woff2' { const url: string; export default url }
declare module '*.mp4' { const url: string; export default url }
declare module '*.m4v' { const url: string; export default url }
declare module '*.m4a' { const url: string; export default url }
declare module '*.mov' { const url: string; export default url }
declare module '*.webm' { const url: string; export default url }
declare module '*.mkv' { const url: string; export default url }
declare module '*.ogg' { const url: string; export default url }
declare module '*.oga' { const url: string; export default url }
declare module '*.ogv' { const url: string; export default url }
declare module '*.opus' { const url: string; export default url }
declare module '*.mp3' { const url: string; export default url }
declare module '*.wav' { const url: string; export default url }
declare module '*.flac' { const url: string; export default url }
declare module '*.aac' { const url: string; export default url }
declare module '*.wasm' { const url: string; export default url }
declare module '*.glb' { const url: string; export default url }
declare module '*.bcmap' { const url: string; export default url }
declare module '*.pfb' { const url: string; export default url }

declare module 'node:ffi' {
  type FFIType =
    | 'void' | 'bool' | 'char' | 'pointer' | 'ptr' | 'string' | 'str' | 'buffer' | 'arraybuffer' | 'function'
    | 'int8' | 'uint8' | 'int16' | 'uint16' | 'int32' | 'uint32' | 'int64' | 'uint64' | 'float32' | 'float64'
    | 'i8' | 'u8' | 'i16' | 'u16' | 'i32' | 'u32' | 'i64' | 'u64' | 'f32' | 'f64' | 'float' | 'double'

  interface FunctionDefinition {
    arguments?: FFIType[]
    /** Defaults to `void`. */
    return?: FFIType
  }

  /** 64-bit integers and pointers are bigints. */
  type ForeignFunction = ((...args: any[]) => any) & { readonly pointer: bigint }

  class DynamicLibrary implements Disposable {
    /** `null` opens the running process. */
    constructor(path: string | null)
    readonly path: string | null
    readonly functions: Record<string, ForeignFunction>
    readonly symbols: Record<string, bigint>
    close(): void
    getFunction(name: string, definition?: FunctionDefinition): ForeignFunction
    getFunctions(definitions?: Record<string, FunctionDefinition>): Record<string, ForeignFunction>
    getSymbol(name: string): bigint
    getSymbols(): Record<string, bigint>
    registerCallback(definition: FunctionDefinition, fn: (...args: any[]) => any): bigint
    registerCallback(fn: (...args: any[]) => any): bigint
    unregisterCallback(address: bigint): void
    refCallback(address: bigint): void
    unrefCallback(address: bigint): void
    [Symbol.dispose](): void
  }

  const suffix: string
  const types: {
    readonly VOID: 'void'; readonly POINTER: 'pointer'; readonly BUFFER: 'buffer'; readonly ARRAY_BUFFER: 'arraybuffer'; readonly FUNCTION: 'function'
    readonly BOOL: 'bool'; readonly CHAR: 'char'; readonly STRING: 'string'; readonly FLOAT: 'float'; readonly DOUBLE: 'double'
    readonly INT_8: 'int8'; readonly UINT_8: 'uint8'; readonly INT_16: 'int16'; readonly UINT_16: 'uint16'; readonly INT_32: 'int32'; readonly UINT_32: 'uint32'
    readonly INT_64: 'int64'; readonly UINT_64: 'uint64'; readonly FLOAT_32: 'float32'; readonly FLOAT_64: 'float64'
  }

  function dlopen(path: string | null, definitions?: Record<string, FunctionDefinition>): { lib: DynamicLibrary; functions: Record<string, ForeignFunction> } & Disposable
  function dlclose(lib: DynamicLibrary): void
  function dlsym(lib: DynamicLibrary, name: string): bigint
  function getRawPointer(source: ArrayBuffer | ArrayBufferView): bigint
  function toArrayBuffer(address: bigint, size: number, copy?: boolean): ArrayBuffer
  function toBuffer(address: bigint, size: number, copy?: boolean): Uint8Array
  /** Reads a NUL-terminated UTF-8 string. */
  function toString(address: bigint): string
  function exportString(value: string, address: bigint, size: number, encoding?: 'utf8' | 'utf16le' | 'latin1'): void
  function exportBuffer(source: Uint8Array, address: bigint, size: number): void
  function exportArrayBuffer(source: ArrayBuffer, address: bigint, size: number): void
  function exportArrayBufferView(source: ArrayBufferView, address: bigint, size: number): void
  /** Throws, as there is no libuv loop. */
  function getCurrentEventLoop(): never

  function getInt8(address: bigint, offset?: number): number
  function getUint8(address: bigint, offset?: number): number
  function getInt16(address: bigint, offset?: number): number
  function getUint16(address: bigint, offset?: number): number
  function getInt32(address: bigint, offset?: number): number
  function getUint32(address: bigint, offset?: number): number
  function getInt64(address: bigint, offset?: number): bigint
  function getUint64(address: bigint, offset?: number): bigint
  function getFloat32(address: bigint, offset?: number): number
  function getFloat64(address: bigint, offset?: number): number
  function setInt8(address: bigint, offset: number, value: number): void
  function setUint8(address: bigint, offset: number, value: number): void
  function setInt16(address: bigint, offset: number, value: number): void
  function setUint16(address: bigint, offset: number, value: number): void
  function setInt32(address: bigint, offset: number, value: number): void
  function setUint32(address: bigint, offset: number, value: number): void
  function setInt64(address: bigint, offset: number, value: bigint): void
  function setUint64(address: bigint, offset: number, value: bigint): void
  function setFloat32(address: bigint, offset: number, value: number): void
  function setFloat64(address: bigint, offset: number, value: number): void
}
