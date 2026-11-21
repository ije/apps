import { Toggle } from "@ui-sdk/react/toggle"
import { Component, memo, type ComponentType, type ReactNode } from "react"
import type { TaggedTile as TileData } from "./types"

class Boundary extends Component<
  { id: string; children: ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null }
  static getDerivedStateFromError(error: Error) {
    return { error }
  }
  render() {
    if (this.state.error) {
      return (
        <p className="tile-error" role="alert">
          {this.props.id} failed: {this.state.error.message}
        </p>
      )
    }
    return this.props.children
  }
}

// Picking a tag rerenders every tile's tags, never its demo.
const Stage = memo(function Stage({
  id,
  Demo,
}: {
  id: string
  Demo: ComponentType
}) {
  return (
    <div className="tile-stage">
      <Boundary id={id}>
        <Demo />
      </Boundary>
    </div>
  )
})

type Props = {
  tile: TileData
  hidden: boolean
  selected: ReadonlySet<string>
  onToggle: (name: string, pressed: boolean) => void
}

export function Tile({ tile, hidden, selected, onToggle }: Props) {
  return (
    <article
      className="tile"
      data-size={tile.size ?? "s"}
      data-tone={tile.tone ?? "white"}
      id={tile.id}
      hidden={hidden}
    >
      <Stage id={tile.id} Demo={tile.Demo} />
      <footer className="tile-copy">
        <p className="tile-title">
          <strong>{tile.title}</strong> {tile.blurb}
        </p>
        <div className="tile-uses" aria-label="Uses">
          {tile.uses.map((name) => (
            <Toggle
              key={name}
              className="tile-use"
              pressed={selected.has(name)}
              onPressedChange={(pressed) => onToggle(name, pressed)}
            >
              {name}
            </Toggle>
          ))}
        </div>
      </footer>
    </article>
  )
}
