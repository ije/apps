import { tiles as collections } from "../tiles/collections"
import { tiles as drag } from "../tiles/drag"
import { tiles as forms } from "../tiles/forms"
import { tiles as gestures } from "../tiles/gestures"
import { tiles as language } from "../tiles/language"
import { tiles as layers } from "../tiles/layers"
import { tiles as locale } from "../tiles/locale"
import { tiles as motion } from "../tiles/motion"
import { tiles as overlays } from "../tiles/overlays"
import { tiles as pickers } from "../tiles/pickers"
import { tiles as records } from "../tiles/records"
import { tiles as reference } from "../tiles/reference"
import { tiles as studio } from "../tiles/studio"
import type { TaggedTile, Tile } from "./types"
import { uses } from "./uses.generated"

/** "Slider" is a component, "createTypeahead" a core helper, "useShortcuts" a hook. */
export const kindOf = (name: string) =>
  /^use[A-Z]/.test(name) ? "hook" : /^[A-Z]/.test(name) ? "component" : "helper"

export const groups: Record<string, Tile[]> = {
  reference,
  forms: [...forms, ...pickers],
  overlays: [...overlays, ...layers],
  collections: [...collections, ...records],
  motion: [...motion, ...gestures],
  language: [...language, ...locale],
  drag,
  studio,
}

// One tile from each file in turn, so neighbors come from different areas.
function interleave(lists: Tile[][]) {
  const out: Tile[] = []
  for (let i = 0; lists.some((list) => i < list.length); i++) {
    for (const list of lists) {
      if (list[i]) {
        out.push(list[i])
      }
    }
  }
  return out
}

const tileCount = new Map<string, number>()
for (const names of Object.values(uses)) {
  for (const name of names) {
    tileCount.set(name, (tileCount.get(name) ?? 0) + 1)
  }
}
const kindOrder = { component: 0, hook: 1, helper: 2 }

// Components first, then hooks and helpers, the rarest first, so a tile shows what sets it apart before what every tile has.
const ordered = (names: string[]) =>
  [...names].sort(
    (a, b) =>
      kindOrder[kindOf(a)] - kindOrder[kindOf(b)] ||
      (tileCount.get(a) ?? 0) - (tileCount.get(b) ?? 0) ||
      a.localeCompare(b)
  )

// The most striking tiles open the page, above the fold, in this order.
const featured = [
  "forms-alerts",
  "overlays-region",
  "collections-book-contents",
  "language-sheet",
]
const rank = (tile: Tile) => {
  const index = featured.indexOf(tile.id)
  return index === -1 ? featured.length : index
}

export const tiles: TaggedTile[] = interleave(Object.values(groups))
  .sort((a, b) => rank(a) - rank(b))
  .map((tile) => ({
    ...tile,
    uses: ordered(uses[tile.id] ?? []),
  }))

export const groupOf = new Map(
  Object.entries(groups).flatMap(([name, list]) =>
    list.map((tile) => [tile.id, name])
  )
)
