const output = document.querySelector('#response')

try {
  const response = await fetch('http://localhost:8089', { signal: AbortSignal.timeout(5000) })
  if (!response.ok) throw new Error(`HTTP ${response.status}`)
  output.textContent = await response.text()
} catch (error) {
  output.textContent = `Request failed: ${error}`
}
