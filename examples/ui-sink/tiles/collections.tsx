import {
  ArrowDownIcon,
  BookmarkSimpleIcon,
  BookOpenTextIcon,
  CaretDownIcon,
  CaretLeftIcon,
  CaretRightIcon,
  CheckIcon,
  FileAudioIcon,
  FileImageIcon,
  FilePdfIcon,
  FileTextIcon,
  FolderIcon,
  FolderOpenIcon,
  MagnifyingGlassIcon,
  MinusIcon,
  MusicNoteIcon,
  PaperPlaneRightIcon,
  PauseIcon,
  PencilSimpleIcon,
  PlayIcon,
  PlusIcon,
  ShuffleIcon,
  SpeakerHighIcon,
  XIcon,
} from "@phosphor-icons/react"
import {
  bindFindable,
  createInlineEdit,
  createStickToBottom,
  createTreeCollection,
  createVirtualizer,
  hiddenAttrs,
  supportsHiddenUntilFound,
  toggleTreeChecked,
  treeCheckedStates,
  treeFromPaths,
  visibleTreeRows,
} from "@ui-sdk/core"
import {
  Accordion,
  AccordionHeader,
  AccordionItem,
  AccordionPanel,
  AccordionTrigger,
} from "@ui-sdk/react/accordion"
import { useAsyncOptions } from "@ui-sdk/react/async-options"
import {
  Autocomplete,
  AutocompleteContent,
  AutocompleteEmpty,
  AutocompleteInput,
  AutocompleteItem,
  AutocompleteLabel,
  AutocompleteList,
  AutocompleteStatus,
} from "@ui-sdk/react/autocomplete"
import { Avatar, AvatarFallback, AvatarImage } from "@ui-sdk/react/avatar"
import {
  Carousel,
  CarouselContent,
  CarouselItem,
  CarouselNext,
  CarouselPrevious,
  CarouselViewport,
} from "@ui-sdk/react/carousel"
import {
  Collapsible,
  CollapsiblePanel,
  CollapsibleTrigger,
} from "@ui-sdk/react/collapsible"
import {
  Combobox,
  ComboboxChip,
  ComboboxChipRemove,
  ComboboxChips,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxItemIndicator,
  ComboboxLabel,
  ComboboxList,
  ComboboxValue,
} from "@ui-sdk/react/combobox"
import {
  Listbox,
  ListboxItem,
  ListboxItemIndicator,
} from "@ui-sdk/react/listbox"
import { Meter, MeterValue } from "@ui-sdk/react/meter"
import {
  Pagination,
  PaginationEllipsis,
  PaginationItem,
  PaginationLink,
  PaginationList,
  PaginationNext,
  PaginationPrevious,
} from "@ui-sdk/react/pagination"
import {
  Progress,
  ProgressIndicator,
  ProgressTrack,
} from "@ui-sdk/react/progress"
import {
  ScrollArea,
  ScrollAreaContent,
  ScrollAreaScrollbar,
  ScrollAreaThumb,
  ScrollAreaViewport,
} from "@ui-sdk/react/scroll-area"
import { Separator } from "@ui-sdk/react/separator"
import {
  Tabs,
  TabsIndicator,
  TabsList,
  TabsPanel,
  TabsTab,
} from "@ui-sdk/react/tabs"
import {
  createTokenInputController,
  TokenInput,
  TokenInputEditor,
  TokenInputToken,
  TokenInputTokenRemove,
  type TokenInputValue,
} from "@ui-sdk/react/token-input"
import {
  Tree,
  TreeGroup,
  TreeItem,
  TreeItemIndicator,
  TreeItemLabel,
  type TreeItemData,
} from "@ui-sdk/react/tree"
import {
  TriggerMenu,
  TriggerMenuContent,
  TriggerMenuEmpty,
  TriggerMenuInput,
  TriggerMenuItem,
  TriggerMenuList,
  TriggerMenuStatus,
  type TriggerMenuDataItem,
} from "@ui-sdk/react/trigger-menu"
import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
} from "react"
import type { Tile } from "../bento/types"
import "./collections.css"

/* Shared helpers */

/** Whether an element is on screen, so timers only run while someone can see them. */
function useInView<T extends Element>() {
  const ref = useRef<T>(null)
  const [inView, setInView] = useState(false)
  useEffect(() => {
    const el = ref.current
    if (!el) {
      return
    }
    const observer = new IntersectionObserver(([entry]) =>
      setInView(entry.isIntersecting)
    )
    observer.observe(el)
    return () => observer.disconnect()
  }, [])
  return [ref, inView] as const
}

const prefersReducedMotion = () =>
  typeof matchMedia !== "undefined" &&
  matchMedia("(prefers-reduced-motion: reduce)").matches

const clock = (seconds: number) =>
  `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, "0")}`

/** Album art and book covers: a gradient block with type, never a photo. */
function Cover({
  hue,
  title,
  size,
  className,
}: {
  hue: number
  title?: string
  size: number
  className?: string
}) {
  return (
    <span
      aria-hidden
      className={`cl-cover${className ? ` ${className}` : ""}`}
      style={{ "--hue": hue, "--size": `${size}px` } as CSSProperties}
    >
      {title && <span className="cl-cover-type">{title}</span>}
    </span>
  )
}

/* 1. Now playing: tabs with a moving indicator over three track lists, and live progress. */

type Track = {
  id: string
  title: string
  artist: string
  secs: number
  hue: number
}

const albumTracks: Track[] = [
  {
    id: "sh-1",
    title: "Harbour Bell",
    artist: "The Low Orchard",
    secs: 214,
    hue: 210,
  },
  {
    id: "sh-2",
    title: "Kelp Forest",
    artist: "The Low Orchard",
    secs: 251,
    hue: 210,
  },
  {
    id: "sh-3",
    title: "Undertow",
    artist: "The Low Orchard",
    secs: 228,
    hue: 210,
  },
  {
    id: "sh-4",
    title: "Gull Song",
    artist: "The Low Orchard",
    secs: 187,
    hue: 210,
  },
  {
    id: "sh-5",
    title: "Salt on the Window",
    artist: "The Low Orchard",
    secs: 302,
    hue: 210,
  },
  {
    id: "sh-6",
    title: "Brine",
    artist: "The Low Orchard",
    secs: 166,
    hue: 210,
  },
  {
    id: "sh-7",
    title: "Pier at Dusk",
    artist: "The Low Orchard",
    secs: 274,
    hue: 210,
  },
  {
    id: "sh-8",
    title: "Lantern Rowing",
    artist: "The Low Orchard",
    secs: 339,
    hue: 210,
  },
]

const queueTracks: Track[] = [
  {
    id: "q-1",
    title: "Glass Meridian",
    artist: "Noor Sato",
    secs: 236,
    hue: 28,
  },
  {
    id: "q-2",
    title: "Copper Weather",
    artist: "Field Hymnal",
    secs: 262,
    hue: 140,
  },
  { id: "q-3", title: "Slow Comet", artist: "Ada Rune", secs: 198, hue: 330 },
  {
    id: "q-4",
    title: "Tin Can Telephone",
    artist: "The Marigolds",
    secs: 171,
    hue: 75,
  },
  {
    id: "q-5",
    title: "Overpass Choir",
    artist: "Sable Junction",
    secs: 289,
    hue: 250,
  },
  {
    id: "q-6",
    title: "Weathervane",
    artist: "Holm & Ash",
    secs: 223,
    hue: 175,
  },
]

const playedTracks: Track[] = [
  { id: "p-1", title: "Paper Kite", artist: "Hollis June", secs: 205, hue: 55 },
  { id: "p-2", title: "Sundial", artist: "Marta Kell", secs: 244, hue: 12 },
  {
    id: "p-3",
    title: "Lemon Grove Static",
    artist: "Cassette Coast",
    secs: 218,
    hue: 95,
  },
  {
    id: "p-4",
    title: "Orbital Lullaby",
    artist: "Fen Okoye",
    secs: 312,
    hue: 230,
  },
]

const everyTrack = [...albumTracks, ...queueTracks, ...playedTracks]

function TrackList({
  label,
  tracks,
  current,
  playing,
  numbered,
  onPick,
}: {
  label: string
  tracks: Track[]
  current: string
  playing: boolean
  numbered?: boolean
  onPick: (id: string) => void
}) {
  return (
    <ScrollArea className="cl-player-area">
      <ScrollAreaViewport aria-label={label} className="cl-player-viewport">
        <ScrollAreaContent>
          <Listbox
            aria-label={label}
            activation="manual"
            value={current}
            onValueChange={(value) =>
              typeof value === "string" && onPick(value)
            }
            className="cl-tracks"
          >
            {tracks.map((track, index) => (
              <ListboxItem
                key={track.id}
                value={track.id}
                label={track.title}
                className="cl-track"
              >
                <span className="cl-track-lead">
                  {track.id === current && playing ? (
                    <SpeakerHighIcon weight="fill" size={16} aria-hidden />
                  ) : numbered ? (
                    index + 1
                  ) : (
                    <Cover hue={track.hue} size={32} />
                  )}
                </span>
                <span className="cl-stack">
                  <span className="cl-track-title">{track.title}</span>
                  <span className="cl-sub">{track.artist}</span>
                </span>
                <span className="cl-mono cl-dim">{clock(track.secs)}</span>
              </ListboxItem>
            ))}
          </Listbox>
        </ScrollAreaContent>
      </ScrollAreaViewport>
      <ScrollAreaScrollbar orientation="vertical" className="cl-scrollbar">
        <ScrollAreaThumb className="cl-thumb" />
      </ScrollAreaScrollbar>
    </ScrollArea>
  )
}

function NowPlaying() {
  const [ref, inView] = useInView<HTMLDivElement>()
  const [current, setCurrent] = useState("sh-3")
  const [elapsed, setElapsed] = useState(71)
  const [playing, setPlaying] = useState(false)
  const track = everyTrack.find((item) => item.id === current) ?? albumTracks[0]
  const done = elapsed >= track.secs
  const running = playing && !done && inView

  useEffect(() => {
    if (!running) {
      return
    }
    const timer = setInterval(() => setElapsed((value) => value + 1), 1000)
    return () => clearInterval(timer)
  }, [running])

  const pick = (id: string) => {
    setCurrent(id)
    setElapsed(0)
    setPlaying(true)
  }
  const toggle = () => {
    if (done) {
      setElapsed(0)
    }
    setPlaying((value) => (done ? true : !value))
  }
  const listProps = { current, playing: playing && !done, onPick: pick }

  return (
    <div ref={ref} className="cl-player">
      <header className="cl-player-head">
        <Cover
          hue={210}
          size={92}
          title="Saltwater Hymns"
          className="cl-player-art"
        />
        <div className="cl-stack">
          <span className="cl-kicker">Album</span>
          <span className="cl-player-title">Saltwater Hymns</span>
          <span className="cl-sub">
            The Low Orchard · 2026 · 8 songs, 32 min
          </span>
        </div>
      </header>
      <Tabs defaultValue="album" className="cl-player-tabs">
        <TabsList aria-label="Track lists" className="cl-tabs">
          <TabsTab value="album" className="cl-tab">
            Album
          </TabsTab>
          <TabsTab value="next" className="cl-tab">
            Up next
          </TabsTab>
          <TabsTab value="played" className="cl-tab">
            Played
          </TabsTab>
          <TabsIndicator className="cl-tab-indicator" />
        </TabsList>
        <TabsPanel value="album" className="cl-tab-panel">
          <TrackList
            label="Album tracks"
            tracks={albumTracks}
            numbered
            {...listProps}
          />
        </TabsPanel>
        <TabsPanel value="next" className="cl-tab-panel">
          <TrackList label="Up next" tracks={queueTracks} {...listProps} />
        </TabsPanel>
        <TabsPanel value="played" className="cl-tab-panel">
          <TrackList
            label="Recently played"
            tracks={playedTracks}
            {...listProps}
          />
        </TabsPanel>
      </Tabs>
      <div className="cl-now">
        <button
          type="button"
          className="cl-play"
          aria-label={playing && !done ? "Pause" : "Play"}
          onClick={toggle}
        >
          {playing && !done ? (
            <PauseIcon weight="fill" size={16} />
          ) : (
            <PlayIcon weight="fill" size={16} />
          )}
        </button>
        <div className="cl-now-main">
          <div className="cl-now-line">
            <span className="cl-track-title">{track.title}</span>
            <span className="cl-sub">{track.artist}</span>
          </div>
          <Progress
            value={Math.min(elapsed, track.secs)}
            max={track.secs}
            aria-label="Playback position"
            getValueLabel={(value) =>
              `${clock(value ?? 0)} of ${clock(track.secs)}`
            }
            className="cl-now-progress"
          >
            <ProgressTrack className="cl-now-track">
              <ProgressIndicator className="cl-now-fill" />
            </ProgressTrack>
          </Progress>
        </div>
        <span className="cl-mono cl-dim cl-now-time">
          {clock(Math.min(elapsed, track.secs))} / {clock(track.secs)}
        </span>
      </div>
    </div>
  )
}

/* 2. Ten thousand tracks, virtualized inside a custom scroll area. */

const LIBRARY_SIZE = 10_000
const libWordsA = [
  "Amber",
  "Hollow",
  "Silver",
  "Velvet",
  "Golden",
  "Faded",
  "Electric",
  "Distant",
  "Gentle",
  "Lucid",
  "Crimson",
  "Tender",
  "Restless",
  "Sunken",
  "Bright",
  "Midnight",
  "Northern",
  "Borrowed",
  "Second",
  "Little",
]
const libWordsB = [
  "Avenue",
  "Signal",
  "Orbit",
  "Garden",
  "Mirror",
  "Season",
  "Echo",
  "Harvest",
  "Satellite",
  "Meadow",
  "Station",
  "Fever",
  "Postcard",
  "Canyon",
  "Parade",
  "Telegram",
  "Ballroom",
  "Relay",
  "Monsoon",
  "Atlas",
  "Hours",
  "Departures",
  "Rooftops",
]
const libFirst = [
  "Noa",
  "Elio",
  "Saskia",
  "Bruno",
  "Yara",
  "Idris",
  "Lotte",
  "Mateo",
  "Ines",
  "Kenji",
  "Aurelie",
  "Tobias",
  "Zadie",
  "Ravi",
  "Freya",
  "Olu",
]
const libLast = [
  "Fenwick",
  "Calder",
  "Moreau",
  "Achebe",
  "Lindqvist",
  "Okada",
  "Varela",
  "Brandauer",
  "Quill",
  "Sorensen",
  "Albright",
  "Nakamura",
  "Duarte",
]

function libraryTrack(index: number) {
  const a = libWordsA[(index * 7) % libWordsA.length]
  const b = libWordsB[(index * 13 + Math.floor(index / 23)) % libWordsB.length]
  const first = libFirst[(index * 5 + Math.floor(index / 16)) % libFirst.length]
  const last = libLast[(index * 3 + Math.floor(index / 13)) % libLast.length]
  return {
    title: `${a} ${b}`,
    artist: `${first} ${last}`,
    secs: 140 + ((index * 37) % 220),
    hue: (index * 47) % 360,
  }
}

function TenThousand() {
  const viewport = useRef<HTMLDivElement>(null)
  const [virtual] = useState(() =>
    createVirtualizer({
      count: () => LIBRARY_SIZE,
      estimate: () => 52,
      overscan: 6,
    })
  )
  const rows = useSyncExternalStore(
    (listener) => virtual.subscribe(listener),
    () => virtual.items(),
    () => virtual.items()
  )
  const [jumped, setJumped] = useState<number | null>(null)

  useEffect(() => {
    const el = viewport.current
    if (!el) {
      return
    }
    return virtual.connect(el)
  }, [virtual])

  const jump = () => {
    const index = Math.floor(Math.random() * LIBRARY_SIZE)
    setJumped(index)
    virtual.scrollToIndex(index, { align: "center" })
  }

  return (
    <div className="cl-lib">
      <header className="cl-lib-head">
        <div className="cl-stack">
          <span className="cl-lib-count">
            10,000 <span>tracks</span>
          </span>
          <span className="cl-mono cl-dim" aria-live="polite">
            {rows.length} rows in the DOM
          </span>
        </div>
        <button type="button" className="b-btn cl-lib-jump" onClick={jump}>
          <ShuffleIcon weight="bold" size={16} aria-hidden />
          Jump
        </button>
      </header>
      <ScrollArea className="cl-lib-area">
        <ScrollAreaViewport
          ref={viewport}
          aria-label="Library"
          className="cl-lib-viewport"
        >
          <ScrollAreaContent>
            <div
              role="list"
              aria-label="Tracks"
              className="cl-lib-box"
              style={{ height: virtual.size() }}
            >
              {rows.map((row) => {
                const track = libraryTrack(row.index)
                return (
                  <div
                    key={row.key}
                    role="listitem"
                    aria-setsize={LIBRARY_SIZE}
                    aria-posinset={row.index + 1}
                    className="cl-lib-row"
                    data-jumped={jumped === row.index || undefined}
                    style={{
                      transform: `translateY(${row.start}px)`,
                      height: row.size,
                    }}
                  >
                    <Cover hue={track.hue} size={34} />
                    <span className="cl-stack">
                      <span className="cl-track-title">{track.title}</span>
                      <span className="cl-sub">{track.artist}</span>
                    </span>
                    <span className="cl-stack cl-lib-meta">
                      <span className="cl-mono cl-dim">
                        {clock(track.secs)}
                      </span>
                      <span className="cl-mono cl-faint">
                        #{(row.index + 1).toLocaleString("en-US")}
                      </span>
                    </span>
                  </div>
                )
              })}
            </div>
          </ScrollAreaContent>
        </ScrollAreaViewport>
        <div className="cl-fade cl-fade-bottom" />
        <ScrollAreaScrollbar orientation="vertical" className="cl-scrollbar">
          <ScrollAreaThumb className="cl-thumb" />
        </ScrollAreaScrollbar>
      </ScrollArea>
    </div>
  )
}

/* 3. A music folder: Tree from file paths, with a preview beside a vertical separator. */

const filePaths = [
  "Music/Albums/Marsh Radio/01 Reedbed.flac",
  "Music/Albums/Marsh Radio/02 Heron Call.flac",
  "Music/Albums/Marsh Radio/03 Peat Fire.flac",
  "Music/Albums/Marsh Radio/cover.jpg",
  "Music/Albums/Quiet Hours/01 Vespers.flac",
  "Music/Albums/Quiet Hours/02 Night Porter.flac",
  "Music/Playlists/Gym rotation.m3u",
  "Audiobooks/The Iron Atlas/Part 1.m4b",
  "Audiobooks/The Iron Atlas/Part 2.m4b",
  "Books/Winter Botany.epub",
  "Books/Field Guide to Moss.pdf",
  "Podcasts/",
]

const fileItems = treeFromPaths(filePaths)

const fileMeta: Record<string, { kind: string; size: string; extra: string }> =
  {
    "Music/Albums/Marsh Radio/01 Reedbed.flac": {
      kind: "FLAC audio",
      size: "38.4 MB",
      extra: "4:12 · 24-bit / 96 kHz",
    },
    "Music/Albums/Marsh Radio/02 Heron Call.flac": {
      kind: "FLAC audio",
      size: "41.9 MB",
      extra: "4:37 · 24-bit / 96 kHz",
    },
    "Music/Albums/Marsh Radio/03 Peat Fire.flac": {
      kind: "FLAC audio",
      size: "29.2 MB",
      extra: "3:15 · 24-bit / 96 kHz",
    },
    "Music/Albums/Marsh Radio/cover.jpg": {
      kind: "JPEG image",
      size: "1.2 MB",
      extra: "1400 × 1400",
    },
    "Music/Albums/Quiet Hours/01 Vespers.flac": {
      kind: "FLAC audio",
      size: "33.0 MB",
      extra: "3:48 · 16-bit / 44.1 kHz",
    },
    "Music/Albums/Quiet Hours/02 Night Porter.flac": {
      kind: "FLAC audio",
      size: "45.7 MB",
      extra: "5:02 · 16-bit / 44.1 kHz",
    },
    "Music/Playlists/Gym rotation.m3u": {
      kind: "Playlist",
      size: "2 KB",
      extra: "42 tracks",
    },
    "Audiobooks/The Iron Atlas/Part 1.m4b": {
      kind: "Audiobook",
      size: "212 MB",
      extra: "6 h 41 min",
    },
    "Audiobooks/The Iron Atlas/Part 2.m4b": {
      kind: "Audiobook",
      size: "188 MB",
      extra: "5 h 57 min",
    },
    "Books/Winter Botany.epub": {
      kind: "EPUB book",
      size: "3.8 MB",
      extra: "312 pages",
    },
    "Books/Field Guide to Moss.pdf": {
      kind: "PDF document",
      size: "18.1 MB",
      extra: "96 pages",
    },
  }

function fileIcon(path: string) {
  if (/\.(flac|m4b|mp3)$/.test(path)) {
    return FileAudioIcon
  }
  if (path.endsWith(".jpg")) {
    return FileImageIcon
  }
  if (path.endsWith(".pdf")) {
    return FilePdfIcon
  }
  if (path.endsWith(".epub")) {
    return BookOpenTextIcon
  }
  return FileTextIcon
}

function findItem(
  items: TreeItemData[],
  value: string
): TreeItemData | undefined {
  for (const item of items) {
    if (item.value === value) {
      return item
    }
    const inside = item.children && findItem(item.children, value)
    if (inside) {
      return inside
    }
  }
}

function FileRows({ items }: { items: TreeItemData[] }) {
  return items.map((item) => {
    const Icon = item.children ? FolderIcon : fileIcon(item.value)
    return (
      <TreeItem key={item.value} value={item.value} className="cl-ftree-item">
        <TreeItemLabel className="cl-ftree-row">
          {(state) => (
            <>
              {item.children ? (
                <TreeItemIndicator className="cl-ftree-caret">
                  <CaretRightIcon weight="bold" size={14} aria-hidden />
                </TreeItemIndicator>
              ) : (
                <span className="cl-ftree-caret" />
              )}
              {item.children && state.expanded ? (
                <FolderOpenIcon
                  weight="fill"
                  size={14}
                  aria-hidden
                  className="cl-ftree-icon"
                />
              ) : (
                <Icon
                  weight="fill"
                  size={14}
                  aria-hidden
                  className="cl-ftree-icon"
                />
              )}
              <span className="cl-ellipsis">{item.label}</span>
            </>
          )}
        </TreeItemLabel>
        {item.children && (
          <TreeGroup className="cl-ftree-group">
            <FileRows items={item.children} />
          </TreeGroup>
        )}
      </TreeItem>
    )
  })
}

function MusicFolder() {
  const [selected, setSelected] = useState<string | null>(
    "Music/Albums/Marsh Radio/02 Heron Call.flac"
  )
  const item = selected ? findItem(fileItems, selected) : undefined
  const meta = selected ? fileMeta[selected] : undefined
  const Icon = item?.children ? FolderIcon : fileIcon(selected ?? "")
  const folder = selected?.split("/").slice(0, -1).join(" / ")
  return (
    <div className="cl-files">
      <Tree
        aria-label="Library folders"
        items={fileItems}
        defaultExpanded={["Music", "Music/Albums", "Music/Albums/Marsh Radio"]}
        value={selected}
        onValueChange={setSelected}
        className="cl-ftree"
      >
        <FileRows items={fileItems} />
      </Tree>
      <Separator orientation="vertical" className="cl-files-sep" />
      <div className="cl-preview" aria-live="polite">
        {item ? (
          <>
            <span className="cl-preview-icon">
              <Icon weight="fill" size={14} aria-hidden />
            </span>
            <span className="cl-preview-name">{item.label}</span>
            <span className="cl-sub">{folder || "Library"}</span>
            <dl className="cl-preview-facts">
              <dt>Kind</dt>
              <dd>{meta?.kind ?? "Folder"}</dd>
              <dt>Size</dt>
              <dd className="cl-mono">
                {meta?.size ?? `${item.children?.length ?? 0} items`}
              </dd>
              {meta && (
                <>
                  <dt>Details</dt>
                  <dd className="cl-mono">{meta.extra}</dd>
                </>
              )}
            </dl>
          </>
        ) : (
          <span className="cl-sub">Select a file to see its details.</span>
        )}
      </div>
    </div>
  )
}

/* 4. What syncs offline: a checkbox tree from createTreeCollection, with mixed folders. */

const syncNodes = [
  { key: "road", parent: null, label: "Road trip" },
  { key: "road/1", parent: "road", label: "Coastline Drive", mb: 9.4 },
  { key: "road/2", parent: "road", label: "Mile Marker", mb: 7.1 },
  { key: "road/3", parent: "road", label: "Two-Lane Blacktop", mb: 11.6 },
  { key: "focus", parent: null, label: "Deep focus" },
  { key: "focus/1", parent: "focus", label: "Grid Lines", mb: 14.3 },
  { key: "focus/2", parent: "focus", label: "Low Hum", mb: 12.5 },
  { key: "focus/3", parent: "focus", label: "Afterimage", mb: 10.9 },
]
const syncTree = createTreeCollection(syncNodes)
const syncInfo = new Map(syncNodes.map((node) => [node.key, node]))

function OfflineSync() {
  const [checked, setChecked] = useState(["road/1", "road/3", "focus"])
  const [expanded, setExpanded] = useState(["road"])
  const state = useMemo(() => treeCheckedStates(syncTree, checked), [checked])
  const visible = syncTree.visible(expanded)
  const total = syncNodes.reduce(
    (sum, node) => sum + (node.mb && state(node.key) === true ? node.mb : 0),
    0
  )
  const toggleOpen = (key: string) =>
    setExpanded((list) =>
      list.includes(key) ? list.filter((item) => item !== key) : [...list, key]
    )

  return (
    <div className="cl-sync">
      <div className="cl-sync-head">
        <span className="cl-label">Keep offline</span>
        <span className="cl-mono">{total.toFixed(1)} MB</span>
      </div>
      <ul className="cl-sync-list" aria-label="Playlists to keep offline">
        {visible.map((key) => {
          const node = syncInfo.get(key)!
          const branch = syncTree.isBranch(key)
          const open = expanded.includes(key)
          const value = state(key)
          return (
            <li
              key={key}
              className="cl-sync-row"
              style={{ "--level": syncTree.level(key) } as CSSProperties}
            >
              {branch ? (
                <button
                  type="button"
                  className="cl-sync-caret"
                  aria-expanded={open}
                  aria-label={`${open ? "Collapse" : "Expand"} ${node.label}`}
                  onClick={() => toggleOpen(key)}
                >
                  <CaretRightIcon weight="bold" size={14} aria-hidden />
                </button>
              ) : (
                <span className="cl-sync-caret" />
              )}
              <button
                type="button"
                role="checkbox"
                aria-checked={value === "indeterminate" ? "mixed" : value}
                className="cl-sync-check"
                onClick={() =>
                  setChecked((list) => toggleTreeChecked(syncTree, list, key))
                }
              >
                <span
                  className="cl-checkbox"
                  data-state={
                    value === "indeterminate" ? "mixed" : value ? "on" : "off"
                  }
                >
                  {value === "indeterminate" ? (
                    <MinusIcon weight="bold" size={14} aria-hidden />
                  ) : value ? (
                    <CheckIcon weight="bold" size={14} aria-hidden />
                  ) : null}
                </span>
                <span
                  className={
                    branch ? "cl-sync-name" : "cl-sync-name cl-sync-leaf"
                  }
                >
                  {node.label}
                </span>
              </button>
              <span className="cl-mono cl-faint">
                {branch
                  ? `${syncTree.children(key).length} songs`
                  : `${node.mb?.toFixed(1)} MB`}
              </span>
            </li>
          )
        })}
      </ul>
    </div>
  )
}

/* 5. A book's contents: Tree rendered as flat rows from visibleTreeRows. */

const contents: TreeItemData[] = [
  {
    value: "part-1",
    label: "Coast",
    children: [
      { value: "ch-1", label: "The Survey Ship" },
      { value: "ch-2", label: "Soundings" },
      { value: "ch-3", label: "A Map of Fog" },
    ],
  },
  {
    value: "part-2",
    label: "Interior",
    children: [
      { value: "ch-4", label: "Salt Roads" },
      { value: "ch-5", label: "The Borderless Room" },
    ],
  },
  {
    value: "part-3",
    label: "Return",
    children: [
      { value: "ch-6", label: "Homecoming Charts" },
      { value: "ch-7", label: "Legend and Scale" },
    ],
  },
]
const contentPages: Record<string, number> = {
  "part-1": 1,
  "ch-1": 3,
  "ch-2": 27,
  "ch-3": 52,
  "part-2": 81,
  "ch-4": 83,
  "ch-5": 118,
  "part-3": 151,
  "ch-6": 153,
  "ch-7": 189,
}
const partNames: Record<string, string> = {
  "part-1": "Part one",
  "part-2": "Part two",
  "part-3": "Part three",
}

function BookContents() {
  const [expanded, setExpanded] = useState(["part-1"])
  const rows = useMemo(() => visibleTreeRows(contents, expanded), [expanded])
  return (
    <div className="cl-toc">
      <div className="cl-toc-head">
        <span className="cl-toc-title">The Cartographer’s Daughter</span>
        <span className="cl-sub">Ines Varga · 204 pages</span>
      </div>
      <Tree
        aria-label="Contents"
        items={contents}
        expanded={expanded}
        onExpandedChange={setExpanded}
        defaultValue="ch-2"
        expandOnPress
        className="cl-toc-tree"
      >
        {rows.map(({ item, level }) => (
          <TreeItem
            key={item.value}
            value={item.value}
            branch={!!item.children}
            className="cl-toc-item"
          >
            <TreeItemLabel className="cl-toc-row" data-level={level}>
              {item.children ? (
                <TreeItemIndicator className="cl-toc-caret">
                  <CaretRightIcon weight="bold" size={14} aria-hidden />
                </TreeItemIndicator>
              ) : (
                <span className="cl-toc-num">{item.value.slice(3)}</span>
              )}
              <span className="cl-ellipsis">
                {item.children ? (
                  <>
                    <span className="cl-toc-part">{partNames[item.value]}</span>{" "}
                    {item.label}
                  </>
                ) : (
                  item.label
                )}
              </span>
              <span className="cl-toc-page">{contentPages[item.value]}</span>
            </TreeItemLabel>
          </TreeItem>
        ))}
      </Tree>
    </div>
  )
}

/* 6. A podcast's seasons in an accordion. */

const seasons = [
  {
    value: "s3",
    label: "Season 3",
    meta: "2026 · 8 episodes",
    episodes: [
      ["Night Market Recipes", "38 min"],
      ["The Ferry Strike", "44 min"],
    ],
  },
  {
    value: "s2",
    label: "Season 2",
    meta: "2025 · 10 episodes",
    episodes: [
      ["Ice Road Choir", "51 min"],
      ["Shortwave Hobbyists", "36 min"],
    ],
  },
  {
    value: "s1",
    label: "Season 1",
    meta: "2024 · 6 episodes",
    episodes: [
      ["Pilot: Slack Water", "29 min"],
      ["The Eel Fishers", "42 min"],
    ],
  },
]

function Seasons() {
  return (
    <div className="cl-seasons">
      <span className="cl-label">Low Tide Radio</span>
      <Accordion defaultValue={["s3"]} className="cl-acc">
        {seasons.map((season) => (
          <AccordionItem
            key={season.value}
            value={season.value}
            className="cl-acc-item"
          >
            <AccordionHeader className="cl-acc-header">
              <AccordionTrigger className="cl-acc-trigger">
                <span className="cl-acc-name">{season.label}</span>
                <span className="cl-sub">{season.meta}</span>
                <CaretDownIcon
                  weight="bold"
                  size={14}
                  aria-hidden
                  className="cl-acc-caret"
                />
              </AccordionTrigger>
            </AccordionHeader>
            <AccordionPanel className="cl-acc-panel">
              <ul className="cl-acc-eps">
                {season.episodes.map(([title, length]) => (
                  <li key={title}>
                    <span className="cl-ellipsis">{title}</span>
                    <span className="cl-mono cl-faint">{length}</span>
                  </li>
                ))}
              </ul>
            </AccordionPanel>
          </AccordionItem>
        ))}
      </Accordion>
    </div>
  )
}

/* 7. Album credits fold away; avatars fall back to initials. */

const portrait = (a: string, b: string) =>
  `data:image/svg+xml,${encodeURIComponent(
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${a}"/><stop offset="1" stop-color="${b}"/></linearGradient></defs><rect width="64" height="64" fill="url(#g)"/><circle cx="32" cy="26" r="11" fill="#fff" fill-opacity=".55"/><rect x="14" y="42" width="36" height="22" rx="11" fill="#fff" fill-opacity=".55"/></svg>`
  )}`

const credits = [
  {
    name: "Mira Okafor",
    role: "Vocals, piano",
    image: portrait("#e0a96d", "#b5523b"),
  },
  { name: "Dev Anand", role: "Producer" },
  { name: "June Halloran", role: "Mixing engineer" },
]

const initialsOf = (name: string) =>
  name
    .split(" ")
    .map((part) => part[0])
    .join("")

function Credits() {
  return (
    <Collapsible defaultOpen className="cl-credits">
      <div className="cl-credits-head">
        <span className="cl-stack">
          <span className="cl-track-title">Paper Lanterns</span>
          <span className="cl-sub">Mira Okafor · 11 songs</span>
        </span>
        <CollapsibleTrigger className="cl-credits-trigger">
          Credits
          <CaretDownIcon weight="bold" size={14} aria-hidden />
        </CollapsibleTrigger>
      </div>
      <CollapsiblePanel className="cl-credits-panel">
        <ul className="cl-credits-list">
          {credits.map((person) => (
            <li key={person.name}>
              <Avatar className="cl-avatar">
                {person.image && (
                  <AvatarImage
                    src={person.image}
                    alt={person.name}
                    className="cl-avatar-img"
                  />
                )}
                <AvatarFallback className="cl-avatar-fallback">
                  {initialsOf(person.name)}
                </AvatarFallback>
              </Avatar>
              <span className="cl-ellipsis">{person.name}</span>
              <span className="cl-sub cl-ellipsis">{person.role}</span>
            </li>
          ))}
        </ul>
      </CollapsiblePanel>
    </Collapsible>
  )
}

/* 8. A podcast backlog: multiple selection kept across pages. */

const episodeTitles = [
  "The last bookshop in Tromsø",
  "Ghostwriters, unmasked",
  "Why paperbacks won",
  "Marginalia of famous readers",
  "A year of reading slowly",
  "The translator’s choice",
  "Libraries after midnight",
  "Fan fiction goes to court",
  "Serial novels, again",
  "The hardest first line",
  "Audiobooks and the ear",
  "Banned in three countries",
  "The editor’s red pen",
  "Reading on the night train",
  "Poetry that sells",
  "Inside a printing plant",
  "Short stories, long tails",
  "The cover redesign wars",
  "Letters to dead authors",
  "Bestseller lists, decoded",
  "The secondhand economy",
  "Bookbinding by hand",
  "Unfinished novels",
  "The reader’s diary",
]
const episodes = episodeTitles.map((title, index) => ({
  id: `ep-${24 - index}`,
  number: 24 - index,
  title,
  length: 28 + ((index * 11) % 30),
}))
const EPISODES_PER_PAGE = 3

function Backlog() {
  const [page, setPage] = useState(1)
  const [picked, setPicked] = useState(["ep-24", "ep-22", "ep-17"])
  const onPage = episodes.slice(
    (page - 1) * EPISODES_PER_PAGE,
    page * EPISODES_PER_PAGE
  )
  const pageIds = onPage.map((episode) => episode.id)
  return (
    <div className="cl-backlog">
      <div className="cl-backlog-head">
        <span className="cl-label">The Margin Notes</span>
        <span className="cl-mono cl-dim" aria-live="polite">
          {picked.length} selected
        </span>
      </div>
      <Listbox<string, true>
        multiple
        aria-label="Episodes to download"
        value={picked.filter((id) => pageIds.includes(id))}
        onValueChange={(value) =>
          setPicked((list) => [
            ...list.filter((id) => !pageIds.includes(id)),
            ...value,
          ])
        }
        className="cl-eps"
      >
        {onPage.map((episode) => (
          <ListboxItem
            key={episode.id}
            value={episode.id}
            label={episode.title}
            className="cl-ep"
          >
            <span className="cl-mono cl-faint">{episode.number}</span>
            <span className="cl-ellipsis">{episode.title}</span>
            <span className="cl-mono cl-faint">{episode.length}m</span>
            <span className="cl-ep-box">
              <ListboxItemIndicator className="cl-ep-check">
                <CheckIcon weight="bold" size={14} aria-hidden />
              </ListboxItemIndicator>
            </span>
          </ListboxItem>
        ))}
      </Listbox>
      <Pagination
        aria-label="Episode pages"
        pageCount={episodes.length / EPISODES_PER_PAGE}
        page={page}
        onPageChange={setPage}
        siblings={0}
        boundaries={1}
        className="cl-pages"
      >
        <PaginationList className="cl-pages-list">
          {({ items }) => (
            <>
              <PaginationItem>
                <PaginationPrevious
                  aria-label="Previous page"
                  className="cl-page"
                >
                  <CaretLeftIcon weight="bold" size={14} aria-hidden />
                </PaginationPrevious>
              </PaginationItem>
              {items.map((item) => (
                <PaginationItem key={item}>
                  {typeof item === "number" ? (
                    <PaginationLink page={item} className="cl-page" />
                  ) : (
                    <PaginationEllipsis className="cl-page-gap" />
                  )}
                </PaginationItem>
              ))}
              <PaginationItem>
                <PaginationNext aria-label="Next page" className="cl-page">
                  <CaretRightIcon weight="bold" size={14} aria-hidden />
                </PaginationNext>
              </PaginationItem>
            </>
          )}
        </PaginationList>
      </Pagination>
    </div>
  )
}

/* 9. Lyrics in a scroll area with edge fades, a custom thumb and a reading line. */

const lyrics = [
  [
    "The freight yard hums in a minor key",
    "Sodium light on the overpass",
    "You fold the map the wrong way round",
    "And we laugh till the windows fog",
  ],
  [
    "Four in the morning, northbound",
    "Every town just a name and a bell",
    "Keep the radio low, keep the heater on",
    "We’ll be home before the gulls",
  ],
  [
    "Somewhere a baker lifts the shutters",
    "Somewhere a ferry starts its turn",
    "I count the poles like rosary beads",
    "You sleep with your boots still on",
  ],
  [
    "Four in the morning, northbound",
    "Nothing ahead but the long white line",
    "If the road runs out, we’ll walk it",
    "We’ll be home before the gulls",
  ],
]

function Lyrics() {
  return (
    <div className="cl-lyrics">
      <div className="cl-lyrics-head">
        <MusicNoteIcon weight="fill" size={16} aria-hidden />
        <span className="cl-ellipsis">
          Four in the Morning, Northbound{" "}
          <span className="cl-dim">· Wren &amp; the Tidelines</span>
        </span>
      </div>
      <ScrollArea className="cl-lyrics-area">
        <ScrollAreaViewport aria-label="Lyrics" className="cl-lyrics-viewport">
          <ScrollAreaContent className="cl-lyrics-content">
            {lyrics.map((verse, index) => (
              <p key={index}>
                {verse.map((line) => (
                  <span key={line}>{line}</span>
                ))}
              </p>
            ))}
          </ScrollAreaContent>
        </ScrollAreaViewport>
        <div className="cl-fade cl-fade-top" />
        <div className="cl-fade cl-fade-bottom" />
        <div className="cl-lyrics-line" aria-hidden />
        <ScrollAreaScrollbar orientation="vertical" className="cl-scrollbar">
          <ScrollAreaThumb className="cl-thumb" />
        </ScrollAreaScrollbar>
      </ScrollArea>
    </div>
  )
}

/* 10. New releases in a carousel of covers. */

const releases = [
  { title: "Soft Machinery", artist: "Lumen Ward", hue: 200 },
  { title: "Blue Hour Club", artist: "Okra & Lime", hue: 245 },
  { title: "Northern Rooms", artist: "Saga Holm", hue: 165 },
  { title: "Midnight Bakery", artist: "The Pastry Saints", hue: 40 },
  { title: "Concrete Garden", artist: "Juno Vale", hue: 120 },
  { title: "Heat Haze", artist: "Marisol", hue: 18 },
  { title: "Second Summer", artist: "Ferris Day", hue: 85 },
  { title: "Dial Tone Love", artist: "Pip Arroyo", hue: 350 },
]

function NewReleases() {
  return (
    <Carousel
      aria-label="New releases"
      slidesPerPage="auto"
      className="cl-carousel"
    >
      <div className="cl-carousel-head">
        <span className="cl-stack">
          <span className="cl-track-title">New this Friday</span>
          <span className="cl-sub">8 albums from artists you follow</span>
        </span>
        <span className="cl-carousel-nav">
          <CarouselPrevious aria-label="Previous albums" className="cl-round">
            <CaretLeftIcon weight="bold" size={16} />
          </CarouselPrevious>
          <CarouselNext aria-label="Next albums" className="cl-round">
            <CaretRightIcon weight="bold" size={16} />
          </CarouselNext>
        </span>
      </div>
      <CarouselViewport className="cl-carousel-viewport">
        <CarouselContent className="cl-carousel-content">
          {releases.map((album) => (
            <CarouselItem
              key={album.title}
              aria-label={`${album.title} by ${album.artist}`}
              className="cl-slide"
            >
              <Cover hue={album.hue} size={108} title={album.title} />
              <span className="cl-slide-title cl-ellipsis">{album.title}</span>
              <span className="cl-sub cl-ellipsis">{album.artist}</span>
            </CarouselItem>
          ))}
        </CarouselContent>
      </CarouselViewport>
    </Carousel>
  )
}

/* 11. Importing a folder: determinate bars fill, an unknown count pulses. */

const importFiles = [
  { id: "dawn", name: "Dawn chorus, Øresund.wav", size: "212 MB", rate: 3 },
  { id: "rain", name: "Rain on a tin roof.wav", size: "96 MB", rate: 5 },
  { id: "photos", name: "Sleeve photos", size: "Folder", rate: 6 },
]
const importStart: Record<string, number | null> = {
  dawn: 62,
  rain: 18,
  photos: null,
}

function FolderImport() {
  const [values, setValues] = useState(importStart)
  const [running, setRunning] = useState(false)
  const complete = importFiles.every((file) => values[file.id] === 100)

  useEffect(() => {
    if (!running) {
      return
    }
    const timer = setInterval(() => {
      setValues((current) => {
        const next = { ...current }
        const audioDone = next.dawn === 100 && next.rain === 100
        for (const file of importFiles) {
          const value = next[file.id]
          if (value === null) {
            next[file.id] = audioDone ? 0 : null
          } else {
            next[file.id] = Math.min(100, value + file.rate)
          }
        }
        return next
      })
    }, 140)
    return () => clearInterval(timer)
  }, [running])

  const press = () => {
    if (complete) {
      setValues(importStart)
      setRunning(false)
    } else {
      setRunning((value) => !value)
    }
  }

  return (
    <div className="cl-import">
      <div className="cl-import-head">
        <span className="cl-stack">
          <span className="cl-track-title">Field Recordings 2025</span>
          <span className="cl-sub">
            {complete
              ? "Imported to Library"
              : running
                ? "Importing…"
                : "Paused"}
          </span>
        </span>
        <button
          type="button"
          className="b-btn"
          data-variant="primary"
          onClick={press}
        >
          {complete ? "Again" : running ? "Pause" : "Resume"}
        </button>
      </div>
      <ul className="cl-import-list">
        {importFiles.map((file) => {
          const value = complete ? 100 : values[file.id]
          return (
            <li key={file.id}>
              <Progress
                value={value}
                aria-label={file.name}
                className="cl-import-progress"
              >
                <span className="cl-import-line">
                  <span className="cl-ellipsis">{file.name}</span>
                  <span className="cl-mono cl-dim">
                    {value === null
                      ? "Counting files"
                      : value === 100
                        ? "Done"
                        : `${value}%`}
                  </span>
                </span>
                <ProgressTrack className="cl-import-track">
                  <ProgressIndicator className="cl-import-fill" />
                </ProgressTrack>
              </Progress>
            </li>
          )
        })}
      </ul>
    </div>
  )
}

/* 12. A yearly reading goal drawn as a ring meter. */

const GOAL = 52

function ReadingGoal() {
  const [read, setRead] = useState(34)
  return (
    <div className="cl-goal">
      <Meter
        value={read}
        max={GOAL}
        aria-label="2026 reading goal"
        format={(value) => String(value)}
        getValueLabel={(value) => `${value} of ${GOAL} books`}
        className="cl-ring"
      >
        <span className="cl-ring-face">
          <MeterValue className="cl-ring-num" />
          <span className="cl-sub">of {GOAL} books</span>
        </span>
      </Meter>
      <div className="cl-goal-side">
        <span className="cl-label">2026 reading goal</span>
        <span className="cl-goal-left">
          {read >= GOAL ? "Goal met" : `${GOAL - read} to go`}
        </span>
        <span className="cl-sub">
          {read >= GOAL
            ? "Every book now is a bonus."
            : "Ahead of pace for December."}
        </span>
        <div className="cl-goal-actions">
          <button
            type="button"
            className="cl-goal-minus"
            aria-label="Remove a book"
            disabled={read <= 0}
            onClick={() => setRead((value) => Math.max(0, value - 1))}
          >
            <MinusIcon weight="bold" size={14} />
          </button>
          <button
            type="button"
            className="b-btn"
            data-variant="primary"
            disabled={read >= GOAL}
            onClick={() => setRead((value) => Math.min(GOAL, value + 1))}
          >
            <PlusIcon weight="bold" size={14} aria-hidden />
            Log a book
          </button>
        </div>
      </div>
    </div>
  )
}

/* 13. @mentions in a book club note. */

const readers = [
  { value: "priya", label: "Priya Raman", hue: 20 },
  { value: "oskar", label: "Oskar Lind", hue: 200 },
  { value: "hana", label: "Hana Mori", hue: 330 },
  { value: "leo", label: "Leo Brandt", hue: 120 },
  { value: "ama", label: "Ama Owusu", hue: 60 },
]
const readerHue = new Map(readers.map((reader) => [reader.value, reader.hue]))

function ClubNote() {
  const id = useId()
  return (
    <div className="cl-club">
      <div className="cl-club-head">
        <label htmlFor={id} className="cl-label">
          Tuesday book club
        </label>
        <span className="cl-sub">The Salt Archive · ch. 9</span>
      </div>
      <TriggerMenu triggers={[{ char: "@", items: readers }]}>
        <TriggerMenuInput
          id={id}
          defaultValue="Finished the lighthouse chapter. "
          placeholder="Type @ to mention a reader"
          className="cl-club-input"
        />
        <TriggerMenuStatus className="cl-sr" />
        <TriggerMenuContent className="b-pop cl-menu">
          <TriggerMenuEmpty className="cl-menu-empty">
            No reader by that name.
          </TriggerMenuEmpty>
          <TriggerMenuList>
            {(state) =>
              (state.items as TriggerMenuDataItem[]).map((item) => (
                <TriggerMenuItem
                  key={item.value}
                  value={item.value}
                  label={item.label}
                  className="b-item cl-menu-item"
                >
                  <span
                    aria-hidden
                    className="cl-initials"
                    style={
                      {
                        "--hue": readerHue.get(item.value) ?? 0,
                      } as CSSProperties
                    }
                  >
                    {initialsOf(item.label)}
                  </span>
                  {item.label}
                </TriggerMenuItem>
              ))
            }
          </TriggerMenuList>
        </TriggerMenuContent>
      </TriggerMenu>
      <span className="cl-sub">Type @ to mention · 5 readers</span>
    </div>
  )
}

/* 14. A question with chapter tokens: TokenInput with a TriggerMenu inside. */

const chapters = [
  { value: "ch-1", label: "Barometer" },
  { value: "ch-2", label: "The Long Fog" },
  { value: "ch-3", label: "Hailstones" },
  { value: "ch-4", label: "Thaw" },
  { value: "ch-5", label: "Lightning Rods" },
  { value: "epilogue", label: "Clear Skies" },
]

const promptStart: TokenInputValue = [
  "Compare ",
  { type: "chapter", value: "ch-2", label: "The Long Fog" },
  " with ",
  { type: "chapter", value: "ch-4", label: "Thaw" },
  " ",
]

function ChapterPrompt() {
  const [prompt] = useState(createTokenInputController)
  const [asked, setAsked] = useState<string | null>(null)
  const send = (value: TokenInputValue) => {
    const count = value.filter((part) => typeof part !== "string").length
    if (!prompt.getLabel().trim()) {
      return
    }
    setAsked(
      count === 1 ? "Asked about 1 chapter." : `Asked about ${count} chapters.`
    )
    prompt.clear()
  }
  return (
    <div className="cl-prompt">
      <span className="cl-label">A Thousand Small Weathers</span>
      <TokenInput
        controller={prompt}
        defaultValue={promptStart}
        onSubmit={send}
        renderToken={(token) => (
          <TokenInputToken className="cl-token">
            <BookmarkSimpleIcon weight="fill" size={14} aria-hidden />
            {token.label}
            <TokenInputTokenRemove className="cl-token-remove">
              <XIcon weight="bold" size={14} aria-hidden />
            </TokenInputTokenRemove>
          </TokenInputToken>
        )}
      >
        <div className="cl-prompt-box">
          <TokenInputEditor
            aria-label="Question about the book"
            placeholder="Ask about a chapter"
            className="cl-prompt-editor"
          />
          <div className="cl-prompt-foot">
            <span className="cl-sub" aria-live="polite">
              {asked ?? "@ adds a chapter"}
            </span>
            <button
              type="button"
              className="cl-send"
              aria-label="Ask"
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => send(prompt.getValue())}
            >
              <PaperPlaneRightIcon weight="fill" size={14} />
            </button>
          </div>
        </div>
        <TriggerMenu
          triggers={[{ char: "@", type: "chapter", items: chapters }]}
        >
          <TriggerMenuContent className="b-pop cl-menu">
            <TriggerMenuEmpty className="cl-menu-empty">
              No chapter matches.
            </TriggerMenuEmpty>
            <TriggerMenuList>
              {(state) =>
                (state.items as TriggerMenuDataItem[]).map((item) => (
                  <TriggerMenuItem
                    key={item.value}
                    value={item.value}
                    label={item.label}
                    className="b-item cl-menu-item"
                  >
                    <BookmarkSimpleIcon weight="fill" size={14} aria-hidden />
                    {item.label}
                  </TriggerMenuItem>
                ))
              }
            </TriggerMenuList>
          </TriggerMenuContent>
        </TriggerMenu>
      </TokenInput>
    </div>
  )
}

/* 15. Book search with results that load as you type. */

type Book = {
  value: string
  label: string
  author: string
  year: number
  hue: number
}

const catalog: Book[] = [
  ["The Quiet Engine", "Rhea Castellan", 2019, 210],
  ["Salt & Saffron", "Omar Haddad", 2021, 35],
  ["A Field Guide to Leaving", "Bea Lindgren", 2017, 150],
  ["The Clockmaker’s Garden", "Eamon Pryce", 2023, 95],
  ["Moth Season", "Ines Tal", 2020, 300],
  ["Lowland Psalms", "Kofi Mensah", 2015, 60],
  ["The Ninth Lantern", "Clara Whitlock", 2022, 15],
  ["Every River Ends", "Arjun Mehta", 2018, 190],
  ["Quarry Light", "Sunniva Berg", 2024, 120],
  ["The Understudy", "Paloma Reyes", 2016, 340],
  ["Small Gods of August", "Theo Nakashima", 2021, 50],
  ["The Weight of Feathers", "Mae Abernathy", 2014, 260],
  ["Station Eleven Minutes", "Joss Kerrigan", 2023, 230],
  ["House of Echoing Tiles", "Leila Farahani", 2019, 175],
  ["Glasswing", "Ronan Voss", 2020, 280],
  ["The Orchard Keeper’s Wife", "Hedda Strand", 2012, 80],
].map(([label, author, year, hue]) => ({
  value: String(label).toLowerCase().replace(/\W+/g, "-"),
  label: String(label),
  author: String(author),
  year: Number(year),
  hue: Number(hue),
}))

function searchCatalog(query: string, { signal }: { signal: AbortSignal }) {
  return new Promise<Book[]>((resolve, reject) => {
    const timer = setTimeout(() => {
      const needle = query.trim().toLowerCase()
      resolve(
        catalog
          .filter(
            (book) =>
              !needle ||
              `${book.label} ${book.author}`.toLowerCase().includes(needle)
          )
          .slice(0, 6)
      )
    }, 450)
    signal.addEventListener("abort", () => {
      clearTimeout(timer)
      reject(signal.reason)
    })
  })
}

function BookSearch() {
  const books = useAsyncOptions(searchCatalog, {
    debounce: 250,
    immediate: true,
  })
  const [chosen, setChosen] = useState<Book>(catalog[0])
  return (
    <div className="cl-search">
      <Autocomplete
        items={books.items}
        filter={false}
        status={books.status}
        onValueChange={(value, details) => {
          books.setQuery(value)
          const match = catalog.find((book) => book.label === value)
          if (match && details.reason === "item-press") {
            setChosen(match)
          }
        }}
      >
        <AutocompleteLabel className="cl-label">Find a book</AutocompleteLabel>
        <div className="cl-search-field">
          <MagnifyingGlassIcon
            weight="bold"
            size={14}
            aria-hidden
            className="cl-search-icon"
          />
          <AutocompleteInput
            placeholder="Title or author"
            className="b-input cl-search-input"
          />
        </div>
        <AutocompleteContent sideOffset={6} className="b-pop cl-search-pop">
          <AutocompleteStatus className="cl-menu-status">
            {(state) =>
              state.status === "error"
                ? "Couldn’t reach the catalog."
                : state.loading
                  ? "Searching…"
                  : `${state.count} ${state.count === 1 ? "book" : "books"}`
            }
          </AutocompleteStatus>
          <AutocompleteEmpty className="cl-menu-empty">
            {(state) =>
              state.empty && !state.loading
                ? `No books match “${state.query}”.`
                : null
            }
          </AutocompleteEmpty>
          <AutocompleteList className="cl-search-list">
            {(state) =>
              (state.items as Book[]).map((book) => (
                <AutocompleteItem
                  key={book.value}
                  value={book.value}
                  label={book.label}
                  className="b-item cl-search-item"
                >
                  <Cover hue={book.hue} size={22} />
                  <span className="cl-ellipsis">{book.label}</span>
                  <span className="cl-sub cl-ellipsis">{book.author}</span>
                </AutocompleteItem>
              ))
            }
          </AutocompleteList>
        </AutocompleteContent>
      </Autocomplete>
      <div className="cl-search-card" aria-live="polite">
        <Cover hue={chosen.hue} size={44} />
        <span className="cl-stack">
          <span className="cl-track-title cl-ellipsis">{chosen.label}</span>
          <span className="cl-sub">
            {chosen.author} · {chosen.year}
          </span>
        </span>
      </div>
    </div>
  )
}

/* 16. Genres for a playlist, as chips in a multi-select combobox. */

const genres = [
  { value: "dream-pop", label: "Dream pop" },
  { value: "post-rock", label: "Post-rock" },
  { value: "shoegaze", label: "Shoegaze" },
  { value: "ambient", label: "Ambient" },
  { value: "jazz-fusion", label: "Jazz fusion" },
  { value: "krautrock", label: "Krautrock" },
  { value: "bossa-nova", label: "Bossa nova" },
  { value: "neo-soul", label: "Neo-soul" },
  { value: "chamber-pop", label: "Chamber pop" },
  { value: "synthwave", label: "Synthwave" },
  { value: "folk", label: "Folk" },
]
const genreLabel = (value: string) =>
  genres.find((genre) => genre.value === value)?.label ?? value

function PlaylistGenres() {
  const [picked, setPicked] = useState(["dream-pop", "post-rock"])
  return (
    <div className="cl-genres">
      <div className="cl-genres-head">
        <Cover hue={265} size={36} />
        <span className="cl-stack">
          <span className="cl-track-title">Night Bus Mix</span>
          <span className="cl-sub">
            {picked.length
              ? `About ${picked.length * 37 + 12} songs match`
              : "Add a genre to fill it"}
          </span>
        </span>
      </div>
      <Combobox
        multiple
        value={picked}
        onValueChange={(value) => setPicked((value as string[]) ?? [])}
        itemToLabel={genreLabel}
      >
        <ComboboxLabel className="cl-label">Genres</ComboboxLabel>
        <ComboboxChips className="cl-chips">
          <ComboboxValue>
            {(state) =>
              (state.value as string[]).map((value, index) => (
                <ComboboxChip key={value} value={value} className="cl-chip">
                  {state.labels[index]}
                  <ComboboxChipRemove
                    aria-label={`Remove ${state.labels[index]}`}
                    className="cl-chip-remove"
                  >
                    <XIcon weight="bold" size={14} aria-hidden />
                  </ComboboxChipRemove>
                </ComboboxChip>
              ))
            }
          </ComboboxValue>
          <ComboboxInput
            placeholder={picked.length ? "Add more" : "Add a genre"}
            className="cl-chips-input"
          />
        </ComboboxChips>
        <ComboboxContent sideOffset={6} className="b-pop cl-genre-pop">
          <ComboboxEmpty className="cl-menu-empty">
            No genre by that name.
          </ComboboxEmpty>
          <ComboboxList>
            {genres.map((genre) => (
              <ComboboxItem
                key={genre.value}
                value={genre.value}
                label={genre.label}
                className="b-item cl-genre-item"
              >
                {genre.label}
                <ComboboxItemIndicator className="cl-genre-check">
                  <CheckIcon weight="bold" size={14} aria-hidden />
                </ComboboxItemIndicator>
              </ComboboxItem>
            ))}
          </ComboboxList>
        </ComboboxContent>
      </Combobox>
    </div>
  )
}

/* 17. A copy log that follows its newest line until you scroll up. */

const logFiles = [
  ["Disc 1/01 Low Season.flac", "41.8 MB"],
  ["Disc 1/02 Paper Moon Motel.flac", "37.2 MB"],
  ["Disc 1/03 Cold Open.flac", "29.6 MB"],
  ["Disc 1/04 Tram Lines.flac", "44.0 MB"],
  ["Disc 2/01 Smoke Signals.flac", "35.5 MB"],
  ["Disc 2/02 The Long Way Down.flac", "48.3 MB"],
  ["Disc 2/03 Tidepool.flac", "31.9 MB"],
  ["Disc 2/04 Allotment.flac", "26.7 MB"],
  ["booklet.pdf", "12.4 MB"],
]

function logLine(index: number) {
  const [name, size] = logFiles[index % logFiles.length]
  const seconds = 14 * 3600 + 2 * 60 + index * 3
  const time = [
    Math.floor(seconds / 3600),
    Math.floor(seconds / 60) % 60,
    seconds % 60,
  ]
    .map((part) => String(part).padStart(2, "0"))
    .join(":")
  const skipped = index % 7 === 5
  return { id: index, time, name, size: skipped ? "exists" : size, skipped }
}

function SyncLog() {
  const [ref, inView] = useInView<HTMLDivElement>()
  const scroller = useRef<HTMLDivElement>(null)
  const [stick] = useState(() => createStickToBottom())
  const status = useSyncExternalStore(
    (listener) => stick.subscribe(listener),
    () => stick.get(),
    () => stick.get()
  )
  const [count, setCount] = useState(9)
  const lines = useMemo(
    () =>
      Array.from({ length: Math.min(count, 120) }, (_, index) =>
        logLine(count - Math.min(count, 120) + index)
      ),
    [count]
  )

  useEffect(() => {
    const el = scroller.current
    if (!el) {
      return
    }
    return stick.connect(el)
  }, [stick])

  useEffect(() => {
    if (!inView) {
      return
    }
    const timer = setInterval(() => setCount((value) => value + 1), 1100)
    return () => clearInterval(timer)
  }, [inView])

  return (
    <div ref={ref} className="cl-log">
      <div className="cl-log-head">
        <span className="cl-log-live" data-live={inView || undefined}>
          Copying to pocket player
        </span>
        <span className="cl-mono cl-dim">{count} files</span>
      </div>
      <div className="cl-log-wrap">
        <div
          ref={scroller}
          className="cl-log-scroller"
          tabIndex={0}
          role="log"
          aria-label="Copy log"
        >
          {lines.map((line) => (
            <div
              key={line.id}
              className="cl-log-line"
              data-skipped={line.skipped || undefined}
            >
              <span>{line.time}</span>
              <span className="cl-ellipsis">{line.name}</span>
              <span>{line.size}</span>
            </div>
          ))}
        </div>
        <button
          type="button"
          className="cl-log-jump"
          data-visible={!status.following || undefined}
          tabIndex={status.following ? -1 : 0}
          aria-hidden={status.following}
          onClick={() =>
            stick.scrollToBottom({
              behavior: prefersReducedMotion() ? "auto" : "smooth",
            })
          }
        >
          <ArrowDownIcon weight="bold" size={14} aria-hidden />
          Jump to latest
        </button>
      </div>
    </div>
  )
}

/* 18. Rename playlists in place with createInlineEdit. */

type RenameState = { editing: string | null; names: Record<string, string> }
type InlineEditContext = Parameters<typeof createInlineEdit<RenameState>>[0]

/** The two calls createInlineEdit makes: get and set, plus subscribe for React. */
function createSmallStore<State extends object>(initial: State) {
  let state = initial
  const listeners = new Set<() => void>()
  return {
    get: () => state,
    set(patch: Partial<State>) {
      state = { ...state, ...patch }
      for (const listener of listeners) {
        listener()
      }
    },
    subscribe(listener: () => void) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
  }
}

const playlistRows = [
  { id: "morning", count: 24, hue: 50 },
  { id: "commute", count: 61, hue: 190 },
  { id: "rainy", count: 18, hue: 230 },
  { id: "kitchen", count: 33, hue: 10 },
]

function RenamePlaylists() {
  const [store] = useState(() =>
    createSmallStore<RenameState>({
      editing: null,
      names: {
        morning: "Morning pages",
        commute: "Commute (long)",
        rainy: "Rainy Sunday",
        kitchen: "Kitchen dancing",
      },
    })
  )
  const [edit] = useState(() =>
    // createInlineEdit only calls store.get and store.set, so a small store stands in for a primitive's.
    createInlineEdit({ store } as unknown as InlineEditContext, {
      key: "editing",
      value: (id) => store.get().names[id],
      commit: (id, text) => {
        const name = text.trim()
        if (!name) {
          return false
        }
        store.set({ names: { ...store.get().names, [id]: name } })
        return true
      },
    })
  )
  const state = useSyncExternalStore(store.subscribe, store.get, store.get)
  const bindInput = useCallback(
    (el: HTMLInputElement) => edit.bindInput(el),
    [edit]
  )
  const hint = useId()
  const list = useRef<HTMLUListElement>(null)
  const lastEdited = useRef<string | null>(null)

  // The button that started editing unmounts while the input shows, so put focus back on its replacement.
  useEffect(() => {
    if (state.editing) {
      lastEdited.current = state.editing
      return
    }
    const id = lastEdited.current
    lastEdited.current = null
    const active = document.activeElement
    if (id && (!active || active === document.body)) {
      list.current
        ?.querySelector<HTMLButtonElement>(`[data-row="${id}"] .cl-rename-name`)
        ?.focus()
    }
  }, [state.editing])

  return (
    <div className="cl-rename">
      <ul ref={list} className="cl-rename-list">
        {playlistRows.map((row) => {
          const name = state.names[row.id]
          return (
            <li
              key={row.id}
              data-row={row.id}
              className="cl-rename-row"
              data-editing={state.editing === row.id || undefined}
            >
              <Cover hue={row.hue} size={30} />
              {state.editing === row.id ? (
                <input
                  ref={bindInput}
                  aria-label={`Rename ${name}`}
                  className="cl-rename-input"
                  onKeyDown={(event) => edit.inputKeydown(event.nativeEvent)}
                  onBlur={(event) => edit.end("focus-out", event.nativeEvent)}
                />
              ) : (
                <button
                  type="button"
                  className="cl-rename-name"
                  aria-describedby={hint}
                  onKeyDown={(event) => edit.keydown(event.nativeEvent, row.id)}
                  onDoubleClick={(event) =>
                    edit.dblclick(event.nativeEvent, row.id)
                  }
                >
                  <span className="cl-ellipsis">{name}</span>
                  <span className="cl-mono cl-faint">{row.count}</span>
                </button>
              )}
              <button
                type="button"
                className="cl-rename-pen"
                aria-label={`Rename ${name}`}
                tabIndex={-1}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => edit.start(row.id)}
              >
                <PencilSimpleIcon weight="fill" size={14} />
              </button>
            </li>
          )
        })}
      </ul>
      <p id={hint} className="cl-sub cl-rename-hint">
        Double-click, Enter or F2 to rename
      </p>
    </div>
  )
}

/* 19. A passage that find in page can open: bindFindable with hiddenAttrs. */

const subscribeNever = () => () => {}
function HiddenPassage() {
  const passage = useRef<HTMLDivElement>(null)
  const [shown, setShown] = useState(false)
  const [found, setFound] = useState(false)
  const shownRef = useRef(shown)
  const supported = useSyncExternalStore(
    subscribeNever,
    supportsHiddenUntilFound,
    () => true
  )

  useEffect(() => {
    shownRef.current = shown
    const el = passage.current
    if (!el) {
      return
    }
    const { hidden } = hiddenAttrs(shown, true)
    if (hidden === undefined) {
      el.removeAttribute("hidden")
    } else {
      el.setAttribute("hidden", hidden)
    }
  }, [shown])

  useEffect(() => {
    const el = passage.current
    if (!el) {
      return
    }
    const findable = bindFindable(el, {
      shown: () => shownRef.current,
      findable: () => true,
      reveal: () => {
        shownRef.current = true
        setFound(true)
        setShown(true)
      },
    })
    return findable
  }, [])

  return (
    <div className="cl-passage">
      <span className="cl-label">Chapter 7 · The Lamplighter’s Ledger</span>
      <p className="cl-passage-text">
        Every evening Maren climbed the ninety steps and lit the lamp by hand.
      </p>
      <div
        ref={passage}
        className="cl-passage-hidden"
        data-found={found || undefined}
      >
        <p className="cl-passage-text">
          Under the logbook she found a brass key, cold as river stone, and a
          second ledger in her mother’s hand.
        </p>
      </div>
      <div className="cl-passage-foot">
        <button
          type="button"
          className="b-btn"
          aria-expanded={shown}
          onClick={() => {
            setFound(false)
            setShown((value) => !value)
          }}
        >
          {shown ? "Hide passage" : "Show passage"}
        </button>
        <span className="cl-sub">
          {found
            ? "Opened by find in page."
            : supported
              ? "Or find “brass key” on this page."
              : "Find in page can’t open it here."}
        </span>
      </div>
    </div>
  )
}

export const tiles: Tile[] = [
  {
    id: "collections-now-playing",
    title: "Now playing.",
    blurb: "Tabs over three lists, with live progress.",
    size: "large",
    Demo: NowPlaying,
  },
  {
    id: "collections-ten-thousand",
    title: "Ten thousand tracks.",
    blurb: "Only the rows in view render.",
    size: "tall",
    tone: "paper",
    Demo: TenThousand,
  },
  {
    id: "collections-music-folder",
    title: "Your music folder.",
    blurb: "A file tree built from plain paths.",
    size: "wide",
    Demo: MusicFolder,
  },
  {
    id: "collections-offline-sync",
    title: "Pick what syncs.",
    blurb: "Mixed when partly checked.",
    tone: "sage",
    Demo: OfflineSync,
  },
  {
    id: "collections-book-contents",
    title: "Contents, flattened.",
    blurb: "Nested chapters rendered as flat rows.",
    tone: "paper",
    Demo: BookContents,
  },
  {
    id: "collections-seasons",
    title: "Season by season.",
    blurb: "One season open at a time.",
    Demo: Seasons,
  },
  {
    id: "collections-credits",
    title: "Read the credits.",
    blurb: "Liner notes fold away; initials stand in.",
    tone: "butter",
    Demo: Credits,
  },
  {
    id: "collections-backlog",
    title: "Queue a backlog.",
    blurb: "Selection holds across every page.",
    Demo: Backlog,
  },
  {
    id: "collections-lyrics",
    title: "Scroll the lyrics.",
    blurb: "Custom thumb, fades at both edges.",
    tone: "ink",
    Demo: Lyrics,
  },
  {
    id: "collections-new-releases",
    title: "Fresh this Friday.",
    blurb: "Swipe, use the arrow keys or the buttons.",
    size: "wide",
    tone: "paper",
    Demo: NewReleases,
  },
  {
    id: "collections-folder-import",
    title: "Import in progress.",
    blurb: "Known sizes fill; unknown ones pulse.",
    tone: "blueprint",
    Demo: FolderImport,
  },
  {
    id: "collections-reading-goal",
    title: "Reading goal.",
    blurb: "A meter drawn as a ring.",
    tone: "signal",
    Demo: ReadingGoal,
  },
  {
    id: "collections-club-note",
    title: "Tag the club.",
    blurb: "Type @ to mention a reader.",
    tone: "sky",
    Demo: ClubNote,
  },
  {
    id: "collections-chapter-prompt",
    title: "Ask about chapters.",
    blurb: "Chapters drop in as tokens.",
    Demo: ChapterPrompt,
  },
  {
    id: "collections-book-search",
    title: "Search the stacks.",
    blurb: "Results load as you type.",
    Demo: BookSearch,
  },
  {
    id: "collections-playlist-genres",
    title: "Tag a playlist.",
    blurb: "Each genre you pick becomes a chip.",
    tone: "paper",
    Demo: PlaylistGenres,
  },
  {
    id: "collections-sync-log",
    title: "Follows the tail.",
    blurb: "New lines stay in view until you scroll.",
    tone: "ink",
    Demo: SyncLog,
  },
  {
    id: "collections-rename",
    title: "Rename in place.",
    blurb: "Enter saves, Escape puts it back.",
    tone: "paper",
    Demo: RenamePlaylists,
  },
  {
    id: "collections-hidden-passage",
    title: "Found when searched.",
    blurb: "Find in page opens the hidden passage.",
    Demo: HiddenPassage,
  },
]
