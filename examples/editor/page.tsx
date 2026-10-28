import React, { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { flushSync } from 'react-dom'
import { Editor } from '@tiptap/core'
import StarterKit from '@tiptap/starter-kit'

function App() {
  const [value, setValue] = useState('Edit Unicode text: 😀 你好')
  return <main>
    <h1>Native editor</h1>
    <p>React controls, Tiptap editing, Canvas text, and an isolated WebKit iframe. Press F12 for developer tools.</p>
    <textarea value={value} onChange={event => setValue(event.target.value)} />
    <p>{value}</p>
    <button onClick={() => editor.chain().focus().toggleBold().run()}>Toggle bold</button>
  </main>
}
flushSync(() => createRoot(document.getElementById('root')!).render(<App />))
const editor = new Editor({ element: document.getElementById('editor')!, extensions: [StarterKit], content: '<p>Hello <strong>Tiptap</strong>. Edit this paragraph.</p>' })
const canvas = document.createElement('canvas')
canvas.width = 600
canvas.height = 44
document.body.append(canvas)
const context = canvas.getContext('2d')!
context.fillStyle = '#182536'
context.fillRect(0, 0, 600, 44)
context.fillStyle = '#ffffff'
context.font = '16px monospace'
context.fillText('Canvas text — 😀 你好', 12, 28)
const frame = document.createElement('iframe')
frame.setAttribute('srcdoc', '<html><body style="font:16px system-ui;background:#edf4ff"><p>Isolated WebKit page</p><input value="Browser input" /></body></html>')
document.body.append(frame)
