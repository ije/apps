// WARNING: a menu that closes any other way must not have this mark, or its item blinks.
const mark = (event: Event) => {
  if (event instanceof KeyboardEvent && event.key !== 'Enter' && event.key !== ' ') return
  ;(event.currentTarget as HTMLElement).dataset.chosen = ''
}

export function chosenItem(item: HTMLElement) {
  item.addEventListener('click', mark)
  item.addEventListener('keydown', mark)
}
