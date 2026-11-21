"use client"

// Before the tiles, whose own rules win where they style the same elements.
import "./bento/bento.css"
import { useEffect, useMemo, useState } from "react"
import { Filter } from "./bento/filter"
import { groupOf, tiles } from "./bento/registry"
import { Tile } from "./bento/tile"

type Mode = "any" | "all"

const listParam = (params: URLSearchParams, key: string) =>
  params.get(key)?.split(",").filter(Boolean) ?? []

function Sink() {
  const [only, setOnly] = useState<string[]>([])
  const [selected, setSelected] = useState<string[]>([])
  const [mode, setMode] = useState<Mode>("any")
  const [ready, setReady] = useState(false)

  // `?only=forms` or `?only=forms-plan-picker,motion-dock` shows a file's tiles
  // or single tiles alone. The page is static, so the URL is read after hydration.
  useEffect(() => {
    const params = new URLSearchParams(location.search)
    setOnly(listParam(params, "only"))
    setSelected(listParam(params, "uses"))
    setMode(params.get("mode") === "all" ? "all" : "any")
    setReady(true)
  }, [])

  useEffect(() => {
    if (!ready) {
      return
    }
    const next = new URLSearchParams(location.search)
    if (selected.length) {
      next.set("uses", selected.join(","))
    } else {
      next.delete("uses")
    }
    if (mode === "all") {
      next.set("mode", "all")
    } else {
      next.delete("mode")
    }
    const search = next.toString()
    history.replaceState(
      history.state,
      "",
      search ? `?${search}` : location.pathname
    )
  }, [ready, selected, mode])

  const pool = useMemo(
    () =>
      only.length
        ? tiles.filter(
            (tile) =>
              only.includes(tile.id) ||
              only.includes(groupOf.get(tile.id) ?? "")
          )
        : tiles,
    [only]
  )
  const chosen = new Set(selected)
  const matches = (tile: (typeof tiles)[number]) =>
    !selected.length
      ? true
      : mode === "all"
        ? selected.every((name) => tile.uses.includes(name))
        : tile.uses.some((name) => chosen.has(name))
  const shown = pool.filter(matches)

  const toggle = (name: string, pressed: boolean) =>
    setSelected((current) =>
      pressed
        ? [...new Set([...current, name])]
        : current.filter((other) => other !== name)
    )

  return (
    <div className="sink">
      <h1 className="sr-only">ui-sdk sink</h1>
      <main className="bento">
        {pool.map((tile) => (
          <Tile
            key={tile.id}
            tile={tile}
            hidden={!matches(tile)}
            selected={chosen}
            onToggle={toggle}
          />
        ))}
        {!shown.length && (
          <p className="bento-empty">No tile uses all of those together.</p>
        )}
      </main>
      <Filter
        selected={selected}
        onSelectedChange={setSelected}
        mode={mode}
        onModeChange={setMode}
        shown={shown.length}
      />
    </div>
  )
}

export { Sink }
