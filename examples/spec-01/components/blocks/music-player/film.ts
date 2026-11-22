import camp from "@/photos/camp.jpg"
import river from "@/photos/river.jpg"
import peaks from "@/photos/peaks.jpg"
import cliffs from "@/photos/cliffs.jpg"

// A short film made of four photos, each slowly pushed in and panned, drawn
// from its timecode so a player can seek, scrub and change speed without a
// video file.
const chapters = [
  { title: "Base camp", start: 0, src: camp, from: [0.5, 0.6], to: [0.42, 0.5] },
  { title: "River", start: 18, src: river, from: [0.35, 0.55], to: [0.55, 0.6] },
  { title: "Ridge", start: 42, src: peaks, from: [0.6, 0.35], to: [0.45, 0.45] },
  { title: "Coast", start: 70, src: cliffs, from: [0.45, 0.5], to: [0.6, 0.55] },
]
const filmLength = 96

const images = chapters.map((chapter) => {
  const image = new Image()
  image.src = chapter.src
  return image
})
const ready = Promise.all(images.map((image) => image.decode().catch(() => undefined)))

function frame(ctx: CanvasRenderingContext2D, w: number, h: number, t: number) {
  const index = chapters.findLastIndex((c) => t >= c.start)
  const chapter = chapters[index]
  const end = chapters[index + 1]?.start ?? filmLength
  const k = Math.min(1, (t - chapter.start) / (end - chapter.start))
  ctx.fillStyle = "#000"
  ctx.fillRect(0, 0, w, h)

  const image = images[index]
  if (image.complete && image.naturalWidth) {
    const scale = Math.max(w / image.naturalWidth, h / image.naturalHeight) * (1.08 + k * 0.12)
    const iw = image.naturalWidth * scale
    const ih = image.naturalHeight * scale
    const fx = chapter.from[0] + (chapter.to[0] - chapter.from[0]) * k
    const fy = chapter.from[1] + (chapter.to[1] - chapter.from[1]) * k
    // Pan toward the focus point, but never past the photo's edges: a gap
    // would show the black underneath as a band along one side.
    const x = Math.min(0, Math.max(w - iw, w / 2 - iw * fx))
    const y = Math.min(0, Math.max(h - ih, h / 2 - ih * fy))
    ctx.drawImage(image, x, y, iw, ih)
  }

  // Each cut fades up from black.
  const fade = Math.max(0, 1 - (t - chapter.start) / 0.8)
  if (fade > 0) {
    ctx.fillStyle = `rgba(0,0,0,${fade})`
    ctx.fillRect(0, 0, w, h)
  }

  ctx.fillStyle = "rgba(255,255,255,0.9)"
  ctx.font = `500 ${Math.max(9, h * 0.04)}px "Spec Mono", monospace`
  ctx.textAlign = "left"
  ctx.textBaseline = "top"
  ctx.fillText(`${String(index + 1).padStart(2, "0")} ${chapter.title.toUpperCase()}`, h * 0.05, h * 0.05)

  if (index === 0 && t < 18) {
    const n = Math.max(0, 10 - Math.floor(t / 1.8))
    ctx.font = `200 ${h * 0.36}px "Spec Mono", monospace`
    ctx.textAlign = "center"
    ctx.textBaseline = "middle"
    ctx.fillStyle = "rgba(255,255,255,0.88)"
    ctx.fillText(n > 0 ? String(n).padStart(2, "0") : "GO", w / 2, h * 0.5)
  }
}

export { chapters, filmLength, frame, ready }
