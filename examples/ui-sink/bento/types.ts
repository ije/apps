import type { ComponentType } from "react"

/** How much of the grid a tile takes: one cell, two columns, two rows, or two by two. */
export type TileSize = "s" | "wide" | "tall" | "large"

/** The tile's ground. Demos read `--tile-*` variables, so they work on every tone. */
export type TileTone =
  "white" | "paper" | "ink" | "blueprint" | "signal" | "sage" | "sky" | "butter"

export type Tile = {
  /** Unique, kebab-case, prefixed by its file, like "forms-plan-picker". */
  id: string
  /** The bold run-in lead, one to four words ending with a period: "Pick a plan." */
  title: string
  /** The gray continuation: one short sentence. */
  blurb?: string
  size?: TileSize
  tone?: TileTone
  Demo: ComponentType
}

/** A tile with the ui-sdk names its code uses, read by scripts/tile-uses.mjs: components like "Slider", helpers like "createTypeahead". */
export type TaggedTile = Tile & { uses: string[] }
