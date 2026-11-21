import { XIcon } from "@phosphor-icons/react"
import {
  SegmentedControl,
  SegmentedControlIndicator,
  SegmentedControlItem,
} from "@ui-sdk/react/segmented-control"
import { useState } from "react"

type Props = {
  selected: string[]
  onSelectedChange: (names: string[]) => void
  mode: "any" | "all"
  onModeChange: (mode: "any" | "all") => void
  shown: number
}

export function Filter({
  selected,
  onSelectedChange,
  mode,
  onModeChange,
  shown,
}: Props) {
  const [names, setNames] = useState(selected)
  // Clearing keeps the last names on the bar while it slides away.
  if (selected.length && selected !== names) {
    setNames(selected)
  }
  return (
    <section
      className="filter"
      aria-label="Filters"
      data-open={selected.length ? "" : undefined}
      inert={!selected.length}
    >
      <p className="filter-count" aria-live="polite">
        {selected.length ? `${shown} ${shown === 1 ? "tile" : "tiles"}` : ""}
      </p>
      <ul className="filter-names">
        {names.map((name) => (
          <li key={name}>
            <button
              type="button"
              className="filter-name"
              aria-label={`Remove ${name}`}
              onClick={() =>
                onSelectedChange(selected.filter((other) => other !== name))
              }
            >
              {name}
              <XIcon size={11} weight="bold" aria-hidden />
            </button>
          </li>
        ))}
      </ul>
      {selected.length > 1 && (
        <SegmentedControl
          className="filter-mode"
          aria-label="Match"
          value={mode}
          onValueChange={(value) => value && onModeChange(value)}
        >
          <SegmentedControlIndicator className="filter-mode-indicator" />
          <SegmentedControlItem value="any">Any</SegmentedControlItem>
          <SegmentedControlItem value="all">All</SegmentedControlItem>
        </SegmentedControl>
      )}
      <button
        type="button"
        className="filter-clear"
        onClick={() => onSelectedChange([])}
      >
        Clear filters
      </button>
    </section>
  )
}
