const button = document.getElementById('resize')
const dot = document.getElementById('dot')
const label = document.getElementById('size')
let large = false

button.addEventListener('click', () => {
  large = !large
  const size = large ? 8 : 4
  dot.setAttribute('class', `h-${size} w-${size} rounded-full bg-mint-500`)
  label.textContent = `h-${size} w-${size}`
})
