import {
  ArchiveIcon,
  ArrowCounterClockwiseIcon,
  ArrowLeftIcon,
  ArrowSquareOutIcon,
  ArrowsClockwiseIcon,
  BellIcon,
  CalendarBlankIcon,
  CaretDownIcon,
  CaretRightIcon,
  CaretUpIcon,
  ChartLineIcon,
  ChatCircleIcon,
  CheckIcon,
  ClockIcon,
  CopyIcon,
  CreditCardIcon,
  DatabaseIcon,
  DotsThreeIcon,
  EyeIcon,
  FileCodeIcon,
  FileIcon,
  FolderIcon,
  GearIcon,
  GitBranchIcon,
  GitPullRequestIcon,
  HouseIcon,
  KeyIcon,
  MagnifyingGlassIcon,
  PencilSimpleIcon,
  ReceiptIcon,
  RocketLaunchIcon,
  TerminalWindowIcon,
  TrashIcon,
  UserPlusIcon,
  UsersIcon,
  WarningIcon,
  XIcon,
  type Icon,
} from "@phosphor-icons/react"
import {
  bindLayer,
  createPopup,
  definePrimitive,
  popupClose,
  popupContent,
  popupState,
  popupTrigger,
  position,
} from "@ui-sdk/core"
import { popoverPreset } from "@ui-sdk/core/popover"
import {
  AlertDialog,
  AlertDialogBackdrop,
  AlertDialogClose,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
  AlertDialogTrigger,
  createConfirm,
} from "@ui-sdk/react/alert-dialog"
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandGroupLabel,
  CommandInput,
  CommandItem,
  CommandItemIcon,
  CommandItemText,
  CommandList,
  CommandPage,
  CommandShortcut,
  createCommandController,
} from "@ui-sdk/react/command"
import {
  createDialogController,
  Dialog,
  DialogBackdrop,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "@ui-sdk/react/dialog"
import {
  Drawer,
  DrawerBackdrop,
  DrawerClose,
  DrawerContent,
  DrawerDescription,
  DrawerHandle,
  DrawerTitle,
  DrawerTrigger,
} from "@ui-sdk/react/drawer"
import {
  HoverCard,
  HoverCardArrow,
  HoverCardContent,
  HoverCardTrigger,
} from "@ui-sdk/react/hover-card"
import {
  Menu,
  MenuCheckboxItem,
  MenuContent,
  MenuContextTrigger,
  MenuGroup,
  MenuGroupLabel,
  MenuItem,
  MenuItemIcon,
  MenuItemIndicator,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
  MenuSub,
  MenuSubContent,
  MenuSubTrigger,
  MenuTrigger,
} from "@ui-sdk/react/menu"
import { Menubar } from "@ui-sdk/react/menubar"
import {
  NavigationMenu,
  NavigationMenuContent,
  NavigationMenuItem,
  NavigationMenuLink,
  NavigationMenuLinkIcon,
  NavigationMenuList,
  NavigationMenuTrigger,
  NavigationMenuViewport,
} from "@ui-sdk/react/navigation-menu"
import {
  Popover,
  PopoverArrow,
  PopoverClose,
  PopoverContent,
  PopoverDescription,
  PopoverTitle,
  PopoverTrigger,
} from "@ui-sdk/react/popover"
import { createParts } from "@ui-sdk/react/primitive"
import { useShortcut, useShortcutHint } from "@ui-sdk/react/shortcuts"
import {
  createToaster,
  Toast,
  ToastAction,
  ToastClose,
  ToastDescription,
  ToastRegion,
  ToastTitle,
} from "@ui-sdk/react/toast"
import {
  Tooltip,
  TooltipArrow,
  TooltipContent,
  TooltipShortcut,
  TooltipTrigger,
} from "@ui-sdk/react/tooltip"
import {
  useEffect,
  useRef,
  useState,
  type MouseEvent,
  type ReactNode,
} from "react"
import type { Tile } from "../bento/types"
import "./overlays.css"

const wait = (ms: number) =>
  new Promise<void>((resolve) => setTimeout(resolve, ms))

function Avatar({
  initials,
  size = "md",
}: {
  initials: string
  size?: "sm" | "md" | "lg"
}) {
  return (
    <span className="ov-avatar" data-size={size} aria-hidden>
      {initials}
    </span>
  )
}

/** Splits a formatted hint into keycaps: "Ctrl+Alt+N" or "⌘⌥N". */
function Keys({ text }: { text: string }) {
  // The hint wraps its keys in bidi isolates (U+2066…U+2069); keep only the visible keys.
  const visible = text.replace(/[\u2066-\u2069]/g, "")
  const keys = visible.includes("+") ? visible.split("+") : [...visible]
  return (
    <span className="ov-keys" aria-hidden>
      {keys.map((key, index) => (
        <kbd key={index} className="ov-kbd">
          {key}
        </kbd>
      ))}
    </span>
  )
}

/* 1. Command palette with pages, always open */

type PaletteEntry = {
  value: string
  label: string
  icon: Icon
  page?: string
  shortcut?: string
  hint?: string
}

const paletteSuggested: PaletteEntry[] = [
  {
    value: "deploy",
    label: "Deploy to production",
    icon: RocketLaunchIcon,
    shortcut: "alt+shift+d",
  },
  {
    value: "rollback",
    label: "Roll back last deploy",
    icon: ArrowCounterClockwiseIcon,
    shortcut: "alt+shift+r",
  },
  { value: "logs", label: "Tail production logs", icon: TerminalWindowIcon },
]

const paletteNavigate: PaletteEntry[] = [
  {
    value: "projects",
    label: "Switch project…",
    icon: FolderIcon,
    page: "projects",
    hint: "4",
  },
  {
    value: "people",
    label: "Assign ENG-506 to…",
    icon: UsersIcon,
    page: "people",
  },
  { value: "invoices", label: "Open invoices", icon: CreditCardIcon },
  { value: "invite", label: "Invite to workspace", icon: UserPlusIcon },
]

const paletteProjects = [
  { value: "atlas-api", detail: "Go · iad1" },
  { value: "web-dashboard", detail: "Next.js · edge" },
  { value: "billing-worker", detail: "Rust · queue" },
  { value: "docs-site", detail: "Astro · static" },
]

const palettePeople = [
  { value: "Priya Raman", initials: "PR", detail: "Backend" },
  { value: "Tomás Ortega", initials: "TO", detail: "Payments" },
  { value: "Mei Lin", initials: "ML", detail: "Design systems" },
]

const paletteResults: Record<string, string> = {
  deploy: "Live on production in 38s",
  rollback: "Rolled back to build 2207",
  logs: "Streaming logs from 6 regions",
  invoices: "Opened 3 unpaid invoices",
  invite: "Invite link copied",
}

const pageTitles: Record<string, string> = {
  projects: "Projects",
  people: "Assign",
}

function PaletteItem({ entry }: { entry: PaletteEntry }) {
  const { value, label, icon: ItemIcon, page, shortcut, hint } = entry
  return (
    <CommandItem
      value={value}
      page={page}
      shortcut={shortcut}
      label={label}
      className="ov-cmd-item"
    >
      <CommandItemIcon className="ov-cmd-icon">
        <ItemIcon size={16} weight="bold" />
      </CommandItemIcon>
      <CommandItemText className="ov-cmd-text">{label}</CommandItemText>
      {hint && <span className="ov-cmd-hint">{hint}</span>}
      {shortcut && <CommandShortcut className="ov-cmd-shortcut" />}
      {page && (
        <CaretRightIcon
          size={16}
          weight="bold"
          className="ov-cmd-caret"
          aria-hidden
        />
      )}
    </CommandItem>
  )
}

function Palette() {
  const [command] = useState(() => createCommandController())
  const [pages, setPages] = useState<string[]>([])
  const [result, setResult] = useState("")
  const [query, setQuery] = useState("")
  const page = pages.at(-1)

  async function run(value: string) {
    if (value === "back") {
      return command.pop()
    }
    if (page === "projects") {
      setResult(`Switched to ${value}`)
    } else if (page === "people") {
      setResult(`ENG-506 assigned to ${value}`)
    } else if (value === "vercel") {
      setResult("Building on ▲…")
      await wait(900)
      setResult("▲ Ready. Deployed to production in 0.9s")
    } else if (value === "deploy") {
      setResult("Deploying…")
      await wait(1200)
      setResult(paletteResults.deploy)
    } else {
      setResult(paletteResults[value] ?? value)
    }
    if (page) {
      setPages([])
    }
  }

  return (
    <Command
      className="ov-cmd"
      controller={command}
      pages={pages}
      onPagesChange={setPages}
      onAction={run}
      onQueryChange={setQuery}
      loop
    >
      <div className="ov-cmd-search">
        <MagnifyingGlassIcon size={16} weight="bold" aria-hidden />
        {page && <span className="ov-cmd-crumb">{pageTitles[page]}</span>}
        <CommandInput
          aria-label="Search commands"
          placeholder={page ? "Filter…" : "Type a command or search…"}
          className="ov-cmd-input"
        />
      </div>
      <CommandEmpty className="ov-cmd-empty">
        Nothing matches that.
      </CommandEmpty>
      <CommandList className="ov-cmd-list">
        <CommandPage>
          <CommandGroup className="ov-cmd-group">
            <CommandGroupLabel className="ov-cmd-label">
              Suggested
            </CommandGroupLabel>
            {paletteSuggested.map((entry) => (
              <PaletteItem key={entry.value} entry={entry} />
            ))}
          </CommandGroup>
          <CommandGroup className="ov-cmd-group">
            <CommandGroupLabel className="ov-cmd-label">
              Navigate
            </CommandGroupLabel>
            {paletteNavigate.map((entry) => (
              <PaletteItem key={entry.value} entry={entry} />
            ))}
          </CommandGroup>
          {query.trim().length >= 3 &&
            "vercel".startsWith(query.trim().toLowerCase()) && (
              <CommandItem
                value="vercel"
                label="vercel --prod"
                className="ov-cmd-item"
              >
                <CommandItemIcon className="ov-cmd-icon">
                  <span className="ov-cmd-vercel" aria-hidden>
                    ▲
                  </span>
                </CommandItemIcon>
                <CommandItemText className="ov-cmd-text ov-cmd-mono">
                  vercel --prod
                </CommandItemText>
              </CommandItem>
            )}
        </CommandPage>
        <CommandPage value="projects">
          {paletteProjects.map((project) => (
            <CommandItem
              key={project.value}
              value={project.value}
              className="ov-cmd-item"
            >
              <CommandItemIcon className="ov-cmd-icon">
                <FolderIcon size={16} weight="bold" />
              </CommandItemIcon>
              <CommandItemText className="ov-cmd-text ov-cmd-mono">
                {project.value}
              </CommandItemText>
              <span className="ov-cmd-hint">{project.detail}</span>
            </CommandItem>
          ))}
        </CommandPage>
        <CommandPage value="people">
          {palettePeople.map((person) => (
            <CommandItem
              key={person.value}
              value={person.value}
              className="ov-cmd-item"
            >
              <Avatar initials={person.initials} size="sm" />
              <CommandItemText className="ov-cmd-text">
                {person.value}
              </CommandItemText>
              <span className="ov-cmd-hint">{person.detail}</span>
            </CommandItem>
          ))}
        </CommandPage>
        {page && (
          <CommandItem value="back" className="ov-cmd-item">
            <CommandItemIcon mirror className="ov-cmd-icon">
              <ArrowLeftIcon size={16} weight="bold" />
            </CommandItemIcon>
            Go back
          </CommandItem>
        )}
      </CommandList>
      <footer className="ov-cmd-foot">
        <output className="ov-cmd-result">
          {result && (
            <span
              key={result}
              className="ov-cmd-done"
              data-busy={result.endsWith("…") ? "" : undefined}
            >
              <CheckIcon size={13} weight="bold" aria-hidden />
              {result}
            </span>
          )}
        </output>
        <span className="ov-cmd-keys">
          <span>
            <kbd className="ov-kbd">↵</kbd> Run
          </span>
          {page && (
            <span>
              <kbd className="ov-kbd">⌫</kbd> Back
            </span>
          )}
        </span>
      </footer>
    </Command>
  )
}

/* 2. Dialog with an async close action */

function Invite() {
  const [email, setEmail] = useState("")
  const [error, setError] = useState("")
  const [sent, setSent] = useState<string[]>([])

  async function send() {
    if (!/^\S+@\S+\.\S+$/.test(email)) {
      setError("Enter a work email address.")
      throw new Error("invalid")
    }
    setError("")
    await wait(900)
  }

  return (
    <div className="ov-team">
      <div className="ov-team-avatars">
        <Avatar initials="JB" size="lg" />
        <Avatar initials="AO" size="lg" />
        <Avatar initials="SR" size="lg" />
        <span className="ov-avatar ov-avatar-more" data-size="lg" aria-hidden>
          +{4 + sent.length}
        </span>
      </div>
      <p className="ov-team-name">Helix team</p>
      <p className="ov-team-meta">
        {sent.length ? `Invite sent to ${sent.at(-1)}` : "7 members · 2 admins"}
      </p>
      <Dialog
        onOpenChange={(open, details) => {
          if (open) {
            setEmail("")
            setError("")
          } else if (details.reason === "action") {
            setSent((list) => [...list, email])
          }
        }}
      >
        <DialogTrigger className="b-btn ov-trigger" data-variant="primary">
          <UserPlusIcon size={16} weight="bold" aria-hidden />
          Invite people
        </DialogTrigger>
        <DialogBackdrop className="ov-backdrop" />
        <DialogContent className="ov-dialog">
          <DialogTitle className="ov-dialog-title">Invite to Helix</DialogTitle>
          <DialogDescription className="ov-dialog-desc">
            They can deploy every Helix project.
          </DialogDescription>
          <label className="ov-field">
            <span className="b-label">Work email</span>
            <input
              className="b-input"
              type="email"
              placeholder="name@helix.dev"
              value={email}
              aria-invalid={error ? true : undefined}
              data-invalid={error ? "" : undefined}
              onChange={(event) => setEmail(event.target.value)}
            />
            {error && <span className="ov-field-error">{error}</span>}
          </label>
          <div className="ov-actions">
            <DialogClose className="b-btn">Not now</DialogClose>
            <DialogClose className="b-btn" data-variant="primary" action={send}>
              {(state) => (state.pending ? "Sending…" : "Send invite")}
            </DialogClose>
          </div>
          <DialogClose className="ov-dialog-x" aria-label="Close">
            <XIcon size={16} weight="bold" />
          </DialogClose>
        </DialogContent>
      </Dialog>
    </div>
  )
}

/* 3. AlertDialog with a pending destructive action */

function DeleteProject() {
  const [deleted, setDeleted] = useState(false)
  const restore = useRef<HTMLButtonElement>(null)
  return (
    <div className="ov-project" data-deleted={deleted || undefined}>
      <div className="ov-project-head">
        <span className="ov-project-icon">
          <ClockIcon size={16} weight="fill" aria-hidden />
        </span>
        <div className="ov-project-text">
          <p className="ov-project-name">legacy-cron</p>
          <p className="ov-project-meta">
            {deleted ? "Deleted just now" : "Last deploy 41 days ago"}
          </p>
        </div>
      </div>
      <AlertDialog
        pendingAnnouncement="Deleting legacy-cron"
        onOpenChange={(open, details) => {
          if (!open && details.reason === "action") {
            setDeleted(true)
          }
        }}
        onOpenChangeComplete={(open) => {
          if (!open) {
            restore.current?.focus()
          }
        }}
      >
        {deleted ? (
          <button
            ref={restore}
            type="button"
            className="b-btn ov-wide-btn"
            onClick={() => setDeleted(false)}
          >
            <ArrowCounterClockwiseIcon size={16} weight="bold" aria-hidden />
            Restore project
          </button>
        ) : (
          <AlertDialogTrigger className="b-btn ov-wide-btn">
            <TrashIcon size={16} weight="bold" aria-hidden />
            Delete project
          </AlertDialogTrigger>
        )}
        <AlertDialogBackdrop className="ov-backdrop" />
        <AlertDialogContent className="ov-dialog ov-alert">
          <span className="ov-alert-icon">
            <WarningIcon size={16} weight="fill" aria-hidden />
          </span>
          <AlertDialogTitle className="ov-dialog-title">
            Delete legacy-cron?
          </AlertDialogTitle>
          <AlertDialogDescription className="ov-dialog-desc">
            Its 12 deployments and both domains go with it. This can’t be
            undone.
          </AlertDialogDescription>
          <div className="ov-actions">
            <AlertDialogClose className="b-btn">Cancel</AlertDialogClose>
            <AlertDialogClose
              value="confirm"
              className="b-btn ov-destroy"
              action={() => wait(1100)}
            >
              {(state) => (state.pending ? "Deleting…" : "Delete project")}
            </AlertDialogClose>
          </div>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

/* 4. createConfirm: await an answer from a row action */

const tokenSeed = [
  { name: "ci-runner", id: "sk_live_…4f2a" },
  { name: "grafana-sync", id: "sk_live_…91cd" },
  { name: "local-dev", id: "sk_test_…07be" },
]

function Tokens() {
  const [confirm] = useState(() => createConfirm<{ name: string }>())
  const [tokens, setTokens] = useState(tokenSeed)
  const reset = useRef<HTMLButtonElement>(null)

  async function revoke(name: string, button: HTMLButtonElement) {
    if (!(await confirm({ name }))) {
      return
    }
    const row = button.closest("li")
    const next = (
      row?.nextElementSibling ?? row?.previousElementSibling
    )?.querySelector("button")
    ;(next ?? reset.current)?.focus()
    setTokens((list) => list.filter((token) => token.name !== name))
  }

  return (
    <div className="ov-tokens">
      <p className="ov-tokens-head">
        <KeyIcon size={14} weight="fill" aria-hidden />
        API tokens
      </p>
      {tokens.length ? (
        <ul className="ov-tokens-list">
          {tokens.map((token) => (
            <li key={token.name} className="ov-token">
              <span className="ov-token-text">
                <span className="ov-token-name">{token.name}</span>
                <span className="ov-token-id">{token.id}</span>
              </span>
              <button
                type="button"
                className="b-btn ov-token-revoke"
                onClick={(event) => revoke(token.name, event.currentTarget)}
                aria-label={`Revoke ${token.name}`}
              >
                Revoke
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <div className="ov-tokens-empty">
          <p>No active tokens</p>
          <button
            ref={reset}
            type="button"
            className="b-btn"
            onClick={() => setTokens(tokenSeed)}
          >
            Restore tokens
          </button>
        </div>
      )}
      <AlertDialog controller={confirm.controller}>
        <AlertDialogBackdrop className="ov-backdrop" />
        <AlertDialogContent<{ name: string }> className="ov-dialog ov-alert">
          {(state) => (
            <>
              <span className="ov-alert-icon">
                <KeyIcon size={14} weight="fill" aria-hidden />
              </span>
              <AlertDialogTitle className="ov-dialog-title">
                Revoke {state.payload?.name}?
              </AlertDialogTitle>
              <AlertDialogDescription className="ov-dialog-desc">
                Anything still using this token starts getting 401s right away.
              </AlertDialogDescription>
              <div className="ov-actions">
                <AlertDialogClose className="b-btn">Keep it</AlertDialogClose>
                <AlertDialogClose value="confirm" className="b-btn ov-destroy">
                  Revoke token
                </AlertDialogClose>
              </div>
            </>
          )}
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

/* 5. createDialogController: open from code and await a value */

const regionList = [
  { id: "iad1", city: "Washington, D.C.", ms: "18 ms" },
  { id: "sfo1", city: "San Francisco", ms: "64 ms" },
  { id: "fra1", city: "Frankfurt", ms: "97 ms" },
  { id: "hnd1", city: "Tokyo", ms: "152 ms" },
]

function Region() {
  const [picker] = useState(() => createDialogController<string, string>())
  const [region, setRegion] = useState("fra1")
  const [moving, setMoving] = useState<string | null>(null)
  const [open, setOpen] = useState(false)
  const current = regionList.find((r) => r.id === region)!

  async function move() {
    const { returnValue } = await picker.open({ payload: region })
    if (!returnValue || returnValue === region) {
      return
    }
    setMoving(returnValue)
    await wait(1400)
    setRegion(returnValue)
    setMoving(null)
  }

  return (
    <div className="ov-region">
      <p className="ov-region-lead">
        <DatabaseIcon size={16} weight="fill" aria-hidden />
        pg-main
      </p>
      <p className="ov-region-city">{moving ? "Migrating…" : current.city}</p>
      <p className="ov-region-meta">
        {moving
          ? `Copying to ${moving}, writes paused`
          : `${current.id} · primary · 2.4 GB`}
      </p>
      <button
        type="button"
        className="b-btn ov-trigger"
        aria-haspopup="dialog"
        data-open={open || undefined}
        disabled={!!moving}
        onClick={move}
      >
        Move region…
      </button>
      <Dialog controller={picker} onOpenChange={setOpen}>
        <DialogBackdrop className="ov-backdrop" />
        <DialogContent<string> className="ov-dialog ov-region-dialog">
          {(state) => (
            <>
              <DialogTitle className="ov-dialog-title">
                Move pg-main
              </DialogTitle>
              <DialogDescription className="ov-dialog-desc">
                Pick the region nearest your traffic.
              </DialogDescription>
              <div className="ov-region-list">
                {regionList.map((r) => (
                  <DialogClose
                    key={r.id}
                    value={r.id}
                    className="ov-region-option"
                    data-current={r.id === state.payload || undefined}
                    aria-current={r.id === state.payload ? "true" : undefined}
                  >
                    <span className="ov-region-id">{r.id}</span>
                    <span className="ov-region-name">{r.city}</span>
                    <span className="ov-region-ms">{r.ms}</span>
                    <CheckIcon
                      size={16}
                      weight="bold"
                      className="ov-region-check"
                      aria-hidden
                    />
                  </DialogClose>
                ))}
              </div>
            </>
          )}
        </DialogContent>
      </Dialog>
    </div>
  )
}

/* 6. Drawer with snap points and a handle */

const logLines = [
  ["00:01", "Cloning acme/payments-gateway"],
  ["00:03", "Restored build cache from build 4820"],
  ["00:05", "Installing 412 packages with pnpm"],
  ["00:19", "Running tsc --build"],
  ["00:31", "src/ledger/settle.ts:88 error TS2322"],
  ["00:31", "  Type 'Cents' is not assignable to type 'number'."],
  ["00:32", "Found 1 error in src/ledger/settle.ts"],
  ["00:32", "Command failed with exit code 2"],
  ["00:48", "Build failed. Previous deploy keeps serving."],
]

function BuildLog() {
  const [status, setStatus] = useState<"failed" | "queued" | "ready">("failed")
  async function retry() {
    await wait(1000)
    setStatus("queued")
    setTimeout(() => setStatus("ready"), 2200)
  }
  const label = { failed: "Build failed", queued: "Rebuilding", ready: "Live" }[
    status
  ]
  return (
    <div className="ov-build">
      <p className="ov-build-status" data-status={status}>
        <span className="ov-dot" aria-hidden />
        {label}
      </p>
      <p className="ov-build-name">payments-gateway</p>
      <p className="ov-build-meta">Build 4821 · 48s</p>
      <Drawer snapPoints={[0.5, 1]} defaultSnapPoint={0.5}>
        <DrawerTrigger className="b-btn ov-trigger">
          <TerminalWindowIcon size={16} weight="bold" aria-hidden />
          View log
        </DrawerTrigger>
        <DrawerBackdrop className="ov-drawer-backdrop" />
        <DrawerContent className="ov-drawer">
          <DrawerHandle aria-label="Resize log" className="ov-handle" />
          <header className="ov-drawer-head">
            <div>
              <DrawerTitle className="ov-dialog-title">Build 4821</DrawerTitle>
              <DrawerDescription className="ov-dialog-desc">
                payments-gateway failed after 48s.
              </DrawerDescription>
            </div>
            <DrawerClose
              className="b-btn"
              data-variant="primary"
              action={retry}
              disabled={status !== "failed"}
            >
              {(state) => (state.pending ? "Queuing…" : "Retry build")}
            </DrawerClose>
          </header>
          <ol className="ov-log">
            {logLines.map(([time, text], index) => (
              <li
                key={index}
                data-error={/error|failed|assignable/i.test(text) || undefined}
              >
                <span className="ov-log-time">{time}</span>
                <span>{text}</span>
              </li>
            ))}
          </ol>
        </DrawerContent>
      </Drawer>
    </div>
  )
}

/* 7. Popover with an arrow and a small form */

const releaseSlots = [
  { id: "tonight", day: "Tonight", time: "22:00" },
  { id: "tomorrow", day: "Tomorrow", time: "07:30" },
  { id: "monday", day: "Monday", time: "10:00" },
]

function Schedule() {
  const [pick, setPick] = useState("tomorrow")
  const [scheduled, setScheduled] = useState<string | null>(null)
  const chosen = releaseSlots.find((slot) => slot.id === scheduled)
  return (
    <div className="ov-release">
      <p className="ov-release-tag">
        <GitPullRequestIcon size={16} weight="fill" aria-hidden />
        Release
      </p>
      <p className="ov-release-version">v2.14.0</p>
      <p className="ov-release-meta">
        {chosen
          ? `Ships ${chosen.day.toLowerCase()} at ${chosen.time}`
          : "12 commits since v2.13.2"}
      </p>
      <Popover>
        <PopoverTrigger className="b-btn ov-trigger">
          <CalendarBlankIcon size={16} weight="bold" aria-hidden />
          {chosen ? "Reschedule" : "Schedule"}
        </PopoverTrigger>
        <PopoverContent
          className="b-pop ov-pop"
          side="bottom"
          sideOffset={10}
          collisionPadding={12}
        >
          <PopoverArrow className="ov-arrow" />
          <PopoverTitle className="ov-pop-title">Ship v2.14.0</PopoverTitle>
          <PopoverDescription className="ov-pop-desc">
            Rolls out to all regions.
          </PopoverDescription>
          <div className="ov-slots" role="radiogroup" aria-label="Release time">
            {releaseSlots.map((slot) => (
              <button
                key={slot.id}
                type="button"
                role="radio"
                aria-checked={pick === slot.id}
                className="ov-slot"
                onClick={() => setPick(slot.id)}
              >
                <span>{slot.day}</span>
                <span className="ov-slot-time">{slot.time}</span>
              </button>
            ))}
          </div>
          <PopoverClose
            className="b-btn ov-pop-submit"
            data-variant="primary"
            onClick={() => setScheduled(pick)}
          >
            Schedule release
          </PopoverClose>
        </PopoverContent>
      </Popover>
    </div>
  )
}

/* 8. Tooltips with shortcut hints, bound with useShortcut */

const diffFiles = [
  { path: "src/api/handlers.ts", add: 24, del: 9 },
  { path: "src/lib/ratelimit.ts", add: 61, del: 0 },
  { path: "test/ratelimit.test.ts", add: 38, del: 2 },
  { path: "package.json", add: 1, del: 1 },
]

function ToolButton({
  label,
  keys,
  onPress,
  pressed,
  children,
}: {
  label: string
  keys: string
  onPress: () => void
  pressed?: boolean
  children: ReactNode
}) {
  return (
    <Tooltip shortcut={keys} delay={300}>
      <TooltipTrigger
        className="ov-tool"
        aria-label={label}
        aria-pressed={pressed}
        onClick={onPress}
      >
        {children}
      </TooltipTrigger>
      <TooltipContent className="ov-tip" sideOffset={8}>
        <TooltipArrow className="ov-tip-arrow" />
        {label}
        <TooltipShortcut className="ov-tip-key" />
      </TooltipContent>
    </Tooltip>
  )
}

function DiffToolbar() {
  const [index, setIndex] = useState(0)
  const [viewed, setViewed] = useState<number[]>([])
  const inside = useRef(false)
  const file = diffFiles[index]
  const prev = () => setIndex((i) => Math.max(0, i - 1))
  const next = () => setIndex((i) => Math.min(diffFiles.length - 1, i + 1))
  const toggle = () =>
    setViewed((list) =>
      list.includes(index) ? list.filter((i) => i !== index) : [...list, index]
    )
  const when = () => inside.current

  useShortcut("k", prev, { when, description: "Previous file" })
  useShortcut("j", next, { when, description: "Next file" })
  useShortcut("v", toggle, { when, description: "Mark file viewed" })

  return (
    <div
      className="ov-diff"
      onFocus={() => (inside.current = true)}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) {
          inside.current = false
        }
      }}
    >
      <div className="ov-diff-bar" role="toolbar" aria-label="Review">
        <ToolButton label="Previous file" keys="k" onPress={prev}>
          <CaretUpIcon size={16} weight="bold" />
        </ToolButton>
        <ToolButton label="Next file" keys="j" onPress={next}>
          <CaretDownIcon size={16} weight="bold" />
        </ToolButton>
        <span className="ov-diff-sep" aria-hidden />
        <ToolButton
          label="Mark viewed"
          keys="v"
          onPress={toggle}
          pressed={viewed.includes(index)}
        >
          <EyeIcon size={16} weight="bold" />
        </ToolButton>
        <span className="ov-diff-count">
          {index + 1} / {diffFiles.length}
        </span>
      </div>
      <p className="ov-diff-path" aria-live="polite">
        {file.path}
      </p>
      <p className="ov-diff-stats">
        <span className="ov-diff-add">+{file.add}</span>
        <span className="ov-diff-del">−{file.del}</span>
        <span className="ov-diff-viewed">{viewed.length} of 4 viewed</span>
      </p>
      <div className="ov-diff-progress" aria-hidden>
        {diffFiles.map((_, i) => (
          <span
            key={i}
            data-viewed={viewed.includes(i) || undefined}
            data-current={i === index || undefined}
          />
        ))}
      </div>
    </div>
  )
}

/* 9. useShortcut with platform-aware hints */

const quickKeys = [
  { keys: "mod+alt+n", label: "New branch", result: "Created feat/usage-caps" },
  {
    keys: "mod+alt+p",
    label: "Deploy preview",
    result: "Preview queued for pr-318",
  },
  {
    keys: "alt+shift+i",
    label: "Invite teammate",
    result: "Invite link ready to paste",
  },
]

function QuickKey({
  item,
  onRun,
  hit,
}: {
  item: (typeof quickKeys)[number]
  onRun: () => void
  hit: boolean
}) {
  const hint = useShortcutHint(item.keys)
  useShortcut(item.keys, onRun, { description: item.label })
  return (
    <button
      type="button"
      className="ov-quick"
      data-hit={hit || undefined}
      aria-keyshortcuts={hint.ariaKeyshortcuts}
      onClick={onRun}
    >
      <span>{item.label}</span>
      <Keys text={hint.text} />
    </button>
  )
}

function QuickKeys() {
  const [last, setLast] = useState<number | null>(null)
  const [flash, setFlash] = useState(0)
  useEffect(() => {
    if (!flash) {
      return
    }
    const id = setTimeout(() => setFlash(0), 650)
    return () => clearTimeout(id)
  }, [flash])
  return (
    <div className="ov-quickkeys">
      {quickKeys.map((item, index) => (
        <QuickKey
          key={item.keys}
          item={item}
          hit={flash > 0 && last === index}
          onRun={() => {
            setLast(index)
            setFlash((n) => n + 1)
          }}
        />
      ))}
      <output className="ov-quick-out">
        {last === null
          ? "Works from anywhere on the page"
          : quickKeys[last].result}
      </output>
    </div>
  )
}

/* 10. HoverCard on a mention */

function Mention() {
  return (
    <article className="ov-comment">
      <header className="ov-comment-head">
        <Avatar initials="RS" />
        <span className="ov-comment-who">Ravi Shah</span>
        <span className="ov-comment-when">4m</span>
      </header>
      <div className="ov-comment-body">
        Retry budgets are per tenant now.{" "}
        <HoverCard delay={250}>
          <HoverCardTrigger
            href="#lena-fischer"
            className="ov-mention"
            onClick={(event: MouseEvent) => event.preventDefault()}
          >
            @lena
          </HoverCardTrigger>
          <HoverCardContent
            className="b-pop ov-card"
            sideOffset={10}
            collisionPadding={12}
          >
            <HoverCardArrow className="ov-arrow" />
            <div className="ov-card-head">
              <Avatar initials="LF" size="lg" />
              <div>
                <div className="ov-card-name">Lena Fischer</div>
                <div className="ov-card-role">Platform · Berlin</div>
              </div>
            </div>
            <div className="ov-card-row">
              <ClockIcon size={14} weight="fill" aria-hidden />
              16:42 local time
            </div>
            <div className="ov-card-row ov-card-oncall">
              <span className="ov-dot" aria-hidden />
              On call until Friday
            </div>
          </HoverCardContent>
        </HoverCard>{" "}
        can you sanity-check the alert thresholds?
      </div>
    </article>
  )
}

/* 11. Row menus with submenus, each trigger wearing a tooltip */

const deployRows = [
  {
    sha: "d8f3a1c",
    message: "Cap webhook backoff at 30s",
    branch: "main",
    status: "Ready",
    age: "2m",
    prod: true,
  },
  {
    sha: "41be09f",
    message: "Spend limits on the billing page",
    branch: "feat/spend-limits",
    status: "Building",
    age: "6m",
    prod: false,
  },
  {
    sha: "9c27e4d",
    message: "Move sessions to Redis 7",
    branch: "infra/redis-7",
    status: "Error",
    age: "1h",
    prod: false,
  },
]

const deployActions: Record<string, (sha: string) => string> = {
  visit: (sha) => `Opened preview for ${sha}`,
  redeploy: (sha) => `Redeploying ${sha}`,
  promote: (sha) => `Promoted ${sha} to production`,
  "copy-url": (sha) => `Copied ${sha}.preview.relay.app`,
  "copy-sha": (sha) => `Copied ${sha}`,
  "copy-branch": () => "Copied branch name",
  logs: (sha) => `Opened logs for ${sha}`,
}

function Deploys() {
  const [note, setNote] = useState("")
  return (
    <div className="ov-deploys">
      <div className="ov-deploys-head">
        <p className="ov-deploys-title">Deployments</p>
        <output className="ov-deploys-note">{note}</output>
      </div>
      <ul className="ov-deploys-list">
        {deployRows.map((row) => (
          <li key={row.sha} className="ov-deploy">
            <span
              className="ov-deploy-status"
              data-status={row.status.toLowerCase()}
            >
              <span className="ov-dot" aria-hidden />
              {row.status}
            </span>
            <span className="ov-deploy-msg">
              <span className="ov-deploy-text">{row.message}</span>
              <span className="ov-deploy-sha">{row.sha}</span>
            </span>
            <span className="ov-deploy-branch">
              <GitBranchIcon size={14} weight="bold" aria-hidden />
              {row.branch}
            </span>
            <span className="ov-deploy-age">{row.age}</span>
            <Menu
              onAction={(value) =>
                setNote(deployActions[value]?.(row.sha) ?? "")
              }
            >
              <Tooltip delay={400}>
                <MenuTrigger
                  render={
                    <TooltipTrigger
                      className="ov-dots"
                      aria-label={`Actions for ${row.sha}`}
                    />
                  }
                >
                  <DotsThreeIcon size={16} weight="bold" />
                </MenuTrigger>
                <TooltipContent className="ov-tip" side="top" sideOffset={6}>
                  More actions
                </TooltipContent>
              </Tooltip>
              <MenuContent
                className="b-pop ov-menu"
                align="end"
                sideOffset={6}
                collisionPadding={12}
              >
                <MenuItem value="visit" className="b-item ov-menu-item">
                  <MenuItemIcon className="ov-menu-icon">
                    <ArrowSquareOutIcon size={14} weight="bold" />
                  </MenuItemIcon>
                  Visit preview
                </MenuItem>
                <MenuItem value="redeploy" className="b-item ov-menu-item">
                  <MenuItemIcon className="ov-menu-icon">
                    <ArrowsClockwiseIcon size={14} weight="bold" />
                  </MenuItemIcon>
                  Redeploy
                </MenuItem>
                <MenuItem
                  value="promote"
                  className="b-item ov-menu-item"
                  disabled={row.prod}
                >
                  <MenuItemIcon className="ov-menu-icon">
                    <RocketLaunchIcon size={14} weight="bold" />
                  </MenuItemIcon>
                  Promote to production
                </MenuItem>
                <MenuSub>
                  <MenuSubTrigger className="b-item ov-menu-item">
                    <MenuItemIcon className="ov-menu-icon">
                      <CopyIcon size={14} weight="bold" />
                    </MenuItemIcon>
                    Copy
                    <CaretRightIcon
                      size={14}
                      weight="bold"
                      className="ov-menu-caret"
                      aria-hidden
                    />
                  </MenuSubTrigger>
                  <MenuSubContent
                    className="b-pop ov-menu"
                    collisionPadding={12}
                  >
                    <MenuItem value="copy-url" className="b-item ov-menu-item">
                      Preview URL
                    </MenuItem>
                    <MenuItem value="copy-sha" className="b-item ov-menu-item">
                      Commit SHA
                    </MenuItem>
                    <MenuItem
                      value="copy-branch"
                      className="b-item ov-menu-item"
                    >
                      Branch name
                    </MenuItem>
                  </MenuSubContent>
                </MenuSub>
                <MenuSeparator className="ov-menu-sep" />
                <MenuItem value="logs" className="b-item ov-menu-item">
                  <MenuItemIcon className="ov-menu-icon">
                    <TerminalWindowIcon size={14} weight="bold" />
                  </MenuItemIcon>
                  Runtime logs
                </MenuItem>
              </MenuContent>
            </Menu>
          </li>
        ))}
      </ul>
    </div>
  )
}

/* 12. Context menu on file rows */

const treeSeed = ["Dockerfile", "deploy.yml", "schema.prisma", "README.md"]

function FileTree() {
  const [files, setFiles] = useState(treeSeed)
  const [note, setNote] = useState("Right-click a file")

  function act(value: string, file: string) {
    if (value === "copy") {
      setNote(`Copied infra/${file}`)
    }
    if (value === "open") {
      setNote(`Opened ${file}`)
    }
    if (value === "duplicate") {
      const dot = file.lastIndexOf(".")
      const copy =
        dot > 0
          ? `${file.slice(0, dot)}-copy${file.slice(dot)}`
          : `${file}-copy`
      setFiles((list) =>
        list.includes(copy)
          ? list
          : list.flatMap((f) => (f === file ? [f, copy] : [f]))
      )
      setNote(`Duplicated ${file}`)
    }
    if (value === "trash") {
      setFiles((list) => list.filter((f) => f !== file))
      setNote(`Moved ${file} to trash`)
    }
  }

  return (
    <div className="ov-tree">
      <p className="ov-tree-head">
        <FolderIcon size={14} weight="fill" aria-hidden />
        infra
        <span className="ov-tree-hint">
          or focus and <kbd className="ov-kbd">⇧F10</kbd>
        </span>
      </p>
      <Menu>
        <ul className="ov-tree-list">
          {files.map((file) => (
            <li key={file}>
              <MenuContextTrigger
                payload={file}
                tabIndex={0}
                className="ov-tree-row"
              >
                <FileCodeIcon size={14} weight="fill" aria-hidden />
                {file}
              </MenuContextTrigger>
            </li>
          ))}
          {files.length === 0 && (
            <li>
              <button
                type="button"
                className="b-btn ov-tree-reset"
                onClick={() => setFiles(treeSeed)}
              >
                Restore files
              </button>
            </li>
          )}
        </ul>
        <MenuContent className="b-pop ov-menu">
          {(state) => {
            const file = String(state.payload ?? "")
            return (
              <MenuGroup>
                <MenuGroupLabel className="ov-menu-label">
                  {file}
                </MenuGroupLabel>
                <MenuItem
                  className="b-item ov-menu-item"
                  onSelect={() => act("open", file)}
                >
                  <MenuItemIcon className="ov-menu-icon">
                    <FileIcon size={14} weight="bold" />
                  </MenuItemIcon>
                  Open
                </MenuItem>
                <MenuItem
                  className="b-item ov-menu-item"
                  onSelect={() => act("copy", file)}
                >
                  <MenuItemIcon className="ov-menu-icon">
                    <CopyIcon size={14} weight="bold" />
                  </MenuItemIcon>
                  Copy path
                </MenuItem>
                <MenuItem
                  className="b-item ov-menu-item"
                  onSelect={() => act("duplicate", file)}
                >
                  <MenuItemIcon className="ov-menu-icon">
                    <FileCodeIcon size={14} weight="bold" />
                  </MenuItemIcon>
                  Duplicate
                </MenuItem>
                <MenuSeparator className="ov-menu-sep" />
                <MenuItem
                  className="b-item ov-menu-item"
                  onSelect={() => act("trash", file)}
                >
                  <MenuItemIcon className="ov-menu-icon">
                    <TrashIcon size={14} weight="bold" />
                  </MenuItemIcon>
                  Move to trash
                </MenuItem>
              </MenuGroup>
            )
          }}
        </MenuContent>
      </Menu>
      <output className="ov-tree-note">{note}</output>
    </div>
  )
}

/* 13. Menu item that opens a dialog through its controller */

function BranchRename() {
  const [rename] = useState(() => createDialogController<undefined, string>())
  const [branch, setBranch] = useState("feat/usage-alerts")
  const [draft, setDraft] = useState(branch)
  const [note, setNote] = useState("3 commits ahead of develop")
  const draftRef = useRef(draft)
  draftRef.current = draft

  async function startRename() {
    setDraft(branch)
    const { returnValue } = await rename.open()
    const clean = draftRef.current.trim().replace(/\s+/g, "-")
    if (returnValue === "save" && clean && clean !== branch) {
      setBranch(clean)
      setNote("Renamed on origin")
    }
  }

  return (
    <div className="ov-branch">
      <div className="ov-branch-card">
        <span className="ov-branch-icon">
          <GitBranchIcon size={16} weight="bold" aria-hidden />
        </span>
        <span className="ov-branch-text">
          <span className="ov-branch-name">{branch}</span>
          <span className="ov-branch-meta">{note}</span>
        </span>
        <span className="ov-branch-pr">
          <GitPullRequestIcon size={16} weight="bold" aria-hidden />
          Draft pull request · 2 reviewers
        </span>
        <Menu
          onAction={(value) => {
            if (value === "copy") {
              setNote("Name copied")
            }
            if (value === "compare") {
              setNote("+148 −31 against develop")
            }
          }}
        >
          <MenuTrigger className="ov-dots" aria-label="Branch actions">
            <DotsThreeIcon size={16} weight="bold" />
          </MenuTrigger>
          <MenuContent className="b-pop ov-menu" align="end" sideOffset={6}>
            <MenuItem className="b-item ov-menu-item" onSelect={startRename}>
              <MenuItemIcon className="ov-menu-icon">
                <PencilSimpleIcon size={16} weight="bold" />
              </MenuItemIcon>
              Rename…
            </MenuItem>
            <MenuItem value="copy" className="b-item ov-menu-item">
              <MenuItemIcon className="ov-menu-icon">
                <CopyIcon size={16} weight="bold" />
              </MenuItemIcon>
              Copy name
            </MenuItem>
            <MenuItem value="compare" className="b-item ov-menu-item">
              <MenuItemIcon className="ov-menu-icon">
                <GitPullRequestIcon size={16} weight="bold" />
              </MenuItemIcon>
              Compare with develop
            </MenuItem>
          </MenuContent>
        </Menu>
      </div>
      <Dialog controller={rename}>
        <DialogBackdrop className="ov-backdrop" />
        <DialogContent className="ov-dialog">
          <DialogTitle className="ov-dialog-title">Rename branch</DialogTitle>
          <DialogDescription className="ov-dialog-desc">
            Open pull requests follow the new name.
          </DialogDescription>
          <label className="ov-field">
            <span className="b-label">Branch name</span>
            <input
              className="b-input ov-mono-input"
              value={draft}
              spellCheck={false}
              onChange={(event) => setDraft(event.target.value)}
            />
          </label>
          <div className="ov-actions">
            <DialogClose className="b-btn">Keep current</DialogClose>
            <DialogClose value="save" className="b-btn" data-variant="primary">
              Rename
            </DialogClose>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  )
}

/* 14. Menubar with submenus, checkbox and radio items */

const codeLines = [
  "export async function GET(req: Request, { params }: Ctx) {",
  "  const project = await db.project.findUnique({ where: { slug: params.slug } })",
  "  if (!project) return notFound()",
  '  return Response.json(project, { headers: cache("5m") })',
]

const menubarResults: Record<string, string> = {
  "new-file": "New file",
  save: "Saved route.ts",
  "recent-routes": "Opened api/projects.ts",
  "recent-app": "Opened web/layout.tsx",
  "recent-tf": "Opened infra/main.tf",
  undo: "Undid change",
  redo: "Redid change",
  find: "Searching project",
  staging: "Shipping to staging",
  release: "Releasing to everyone",
  revert: "Reverted to 1.8.3",
}

function MenuShell({
  label,
  children,
}: {
  label: string
  children: ReactNode
}) {
  return (
    <>
      <MenuTrigger className="ov-mb-trigger">{label}</MenuTrigger>
      <MenuContent
        className="b-pop ov-menu"
        sideOffset={6}
        collisionPadding={12}
      >
        {children}
      </MenuContent>
    </>
  )
}

function CheckItem({
  value,
  checked,
  onChange,
  children,
}: {
  value: string
  checked: boolean
  onChange: (checked: boolean) => void
  children: ReactNode
}) {
  return (
    <MenuCheckboxItem
      value={value}
      checked={checked}
      onCheckedChange={onChange}
      className="b-item ov-menu-item"
    >
      <MenuItemIndicator keepMounted className="ov-check">
        <CheckIcon size={14} weight="bold" />
      </MenuItemIndicator>
      {children}
    </MenuCheckboxItem>
  )
}

function Editor() {
  const [minimap, setMinimap] = useState(true)
  const [wrap, setWrap] = useState(false)
  const [numbers, setNumbers] = useState(true)
  const [density, setDensity] = useState("comfortable")
  const [last, setLast] = useState("route.ts")
  const onAction = (value: string) => setLast(menubarResults[value] ?? value)

  return (
    <div className="ov-ide">
      <div className="ov-ide-bar">
        <span className="ov-wordmark">relay</span>
        <Menubar aria-label="Editor" className="ov-menubar">
          <Menu onAction={onAction}>
            <MenuShell label="File">
              <MenuItem
                value="new-file"
                shortcut="mod+n"
                className="b-item ov-menu-item"
              >
                New file
              </MenuItem>
              <MenuSub>
                <MenuSubTrigger className="b-item ov-menu-item">
                  Open recent
                  <CaretRightIcon
                    size={14}
                    weight="bold"
                    className="ov-menu-caret"
                    aria-hidden
                  />
                </MenuSubTrigger>
                <MenuSubContent className="b-pop ov-menu" collisionPadding={12}>
                  <MenuItem
                    value="recent-routes"
                    className="b-item ov-menu-item ov-mono-item"
                  >
                    api/projects.ts
                  </MenuItem>
                  <MenuItem
                    value="recent-app"
                    className="b-item ov-menu-item ov-mono-item"
                  >
                    web/layout.tsx
                  </MenuItem>
                  <MenuItem
                    value="recent-tf"
                    className="b-item ov-menu-item ov-mono-item"
                  >
                    infra/main.tf
                  </MenuItem>
                </MenuSubContent>
              </MenuSub>
              <MenuSeparator className="ov-menu-sep" />
              <MenuItem
                value="save"
                shortcut="mod+s"
                className="b-item ov-menu-item"
              >
                Save
              </MenuItem>
            </MenuShell>
          </Menu>
          <Menu onAction={onAction}>
            <MenuShell label="Edit">
              <MenuItem
                value="undo"
                shortcut="mod+z"
                className="b-item ov-menu-item"
              >
                Undo
              </MenuItem>
              <MenuItem
                value="redo"
                shortcut="mod+shift+z"
                className="b-item ov-menu-item"
              >
                Redo
              </MenuItem>
              <MenuSeparator className="ov-menu-sep" />
              <MenuItem
                value="find"
                shortcut="mod+shift+f"
                className="b-item ov-menu-item"
              >
                Find in project
              </MenuItem>
            </MenuShell>
          </Menu>
          <Menu>
            <MenuShell label="View">
              <CheckItem
                value="minimap"
                checked={minimap}
                onChange={setMinimap}
              >
                Minimap
              </CheckItem>
              <CheckItem value="wrap" checked={wrap} onChange={setWrap}>
                Word wrap
              </CheckItem>
              <CheckItem
                value="numbers"
                checked={numbers}
                onChange={setNumbers}
              >
                Line numbers
              </CheckItem>
              <MenuSeparator className="ov-menu-sep" />
              <MenuRadioGroup value={density} onValueChange={setDensity}>
                <MenuGroupLabel className="ov-menu-label">
                  Density
                </MenuGroupLabel>
                {["Compact", "Comfortable"].map((option) => (
                  <MenuRadioItem
                    key={option}
                    value={option.toLowerCase()}
                    className="b-item ov-menu-item"
                  >
                    <span className="ov-check">
                      <MenuItemIndicator className="ov-radio-dot" />
                    </span>
                    {option}
                  </MenuRadioItem>
                ))}
              </MenuRadioGroup>
            </MenuShell>
          </Menu>
          <Menu onAction={onAction}>
            <MenuShell label="Deploy">
              <MenuItem value="staging" className="b-item ov-menu-item">
                Ship to staging
              </MenuItem>
              <MenuItem value="release" className="b-item ov-menu-item">
                Release to everyone
              </MenuItem>
              <MenuSeparator className="ov-menu-sep" />
              <MenuItem value="revert" className="b-item ov-menu-item">
                Revert last release…
              </MenuItem>
            </MenuShell>
          </Menu>
        </Menubar>
        <output className="ov-ide-status">{last}</output>
      </div>
      <div
        className="ov-code"
        data-density={density}
        data-wrap={wrap || undefined}
      >
        <ol className="ov-code-lines" data-numbers={numbers || undefined}>
          {codeLines.map((line, index) => (
            <li key={index}>
              <code>{line}</code>
            </li>
          ))}
        </ol>
        {minimap && (
          <div className="ov-minimap" aria-hidden>
            {[86, 100, 44, 78, 30, 62, 92, 50].map((width, index) => (
              <span key={index} style={{ width: `${width}%` }} />
            ))}
          </div>
        )}
      </div>
    </div>
  )
}

/* 15. Vertical NavigationMenu in a sidebar */

type SideEntry = { name: string; detail: string }

const sideProjects: SideEntry[] = [
  { name: "storefront", detail: "Production · 3 regions" },
  { name: "inventory-sync", detail: "Cron · every 5 min" },
  { name: "partner-portal", detail: "Preview · pr-77" },
  { name: "data-warehouse", detail: "Paused" },
]

const sideDeploys: SideEntry[] = [
  { name: "Production", detail: "Live since 08:14" },
  { name: "Preview", detail: "11 open branches" },
  { name: "Rollbacks", detail: "Last one Tuesday" },
]

const sideTeam: SideEntry[] = [
  { name: "Members", detail: "14 people" },
  { name: "Roles and access", detail: "4 roles" },
  { name: "Invitations", detail: "3 pending" },
]

function SideLink({
  id,
  active,
  onGo,
  children,
}: {
  id: string
  active: string
  onGo: (id: string) => void
  children: ReactNode
}) {
  return (
    <NavigationMenuLink
      href={`#${id}`}
      current={active === id}
      className="ov-side-link"
      onClick={(event: MouseEvent) => {
        event.preventDefault()
        onGo(id)
      }}
    >
      {children}
    </NavigationMenuLink>
  )
}

function Sidebar() {
  const [active, setActive] = useState("overview")
  const top: { id: string; label: string; icon: Icon }[] = [
    { id: "overview", label: "Overview", icon: HouseIcon },
    { id: "usage", label: "Usage", icon: ChartLineIcon },
    { id: "settings", label: "Settings", icon: GearIcon },
  ]
  const link = ({ id, label, icon: LinkIcon }: (typeof top)[number]) => (
    <NavigationMenuItem key={id}>
      <SideLink id={id} active={active} onGo={setActive}>
        <NavigationMenuLinkIcon className="ov-side-icon">
          <LinkIcon size={16} weight="bold" />
        </NavigationMenuLinkIcon>
        {label}
      </SideLink>
    </NavigationMenuItem>
  )
  const section = (
    value: string,
    label: string,
    SectionIcon: Icon,
    items: SideEntry[]
  ) => (
    <NavigationMenuItem value={value}>
      <NavigationMenuTrigger
        className="ov-side-link ov-side-trigger"
        data-current={items.some((item) => item.name === active) || undefined}
      >
        <span className="ov-side-icon">
          <SectionIcon size={16} weight="bold" aria-hidden />
        </span>
        {label}
        <CaretRightIcon
          size={16}
          weight="bold"
          className="ov-side-caret"
          aria-hidden
        />
      </NavigationMenuTrigger>
      <NavigationMenuContent className="ov-side-panel">
        <p className="ov-side-panel-title">{label}</p>
        <ul>
          {items.map((item) => (
            <li key={item.name}>
              <SideLink id={item.name} active={active} onGo={setActive}>
                <span className="ov-side-panel-name">{item.name}</span>
                <span className="ov-side-panel-detail">{item.detail}</span>
              </SideLink>
            </li>
          ))}
        </ul>
      </NavigationMenuContent>
    </NavigationMenuItem>
  )

  return (
    <div className="ov-side">
      <div className="ov-side-ws">
        <span className="ov-side-ws-text">
          <span className="ov-side-ws-name">Northwind</span>
          <span className="ov-side-ws-plan">Pro workspace</span>
        </span>
      </div>
      <NavigationMenu
        aria-label="Workspace"
        orientation="vertical"
        className="ov-side-nav"
        delay={120}
      >
        <NavigationMenuList className="ov-side-list">
          {link(top[0])}
          {section("projects", "Projects", FolderIcon, sideProjects)}
          {section("deploys", "Deployments", RocketLaunchIcon, sideDeploys)}
          {section("team", "Team", UsersIcon, sideTeam)}
          {link(top[1])}
          {link(top[2])}
        </NavigationMenuList>
        <NavigationMenuViewport
          className="ov-side-viewport"
          sideOffset={12}
          align="start"
          collisionPadding={12}
        />
      </NavigationMenu>
      <div className="ov-side-user">
        <Avatar initials="DA" />
        <span className="ov-side-ws-text">
          <span className="ov-side-user-name">Diego Alvarez</span>
          <span className="ov-side-user-role">Owner</span>
        </span>
      </div>
    </div>
  )
}

/* 16. Stacked toasts with an Undo action */

const inboxSeed: { id: string; icon: Icon; title: string }[] = [
  {
    id: "approve",
    icon: GitPullRequestIcon,
    title: "Noor Haddad approved pr-312",
  },
  { id: "invoice", icon: ReceiptIcon, title: "Invoice INV-2041 is due Friday" },
  {
    id: "staging",
    icon: RocketLaunchIcon,
    title: "Staging deploy finished in 52s",
  },
]

function Inbox() {
  const [toaster] = useState(() => createToaster({ duration: 6000 }))
  const [items, setItems] = useState(inboxSeed)
  useEffect(
    () => () => {
      toaster.dismiss()
    },
    [toaster]
  )

  function restore(id: string) {
    setItems((list) =>
      inboxSeed.filter(
        (item) => item.id === id || list.some((i) => i.id === item.id)
      )
    )
  }

  function archive(item: (typeof inboxSeed)[number]) {
    setItems((list) => list.filter((i) => i.id !== item.id))
    toaster("Archived", {
      id: item.id,
      description: item.title,
      action: {
        label: "Undo",
        onAction: () => restore(item.id),
        dismiss: true,
      },
    })
  }

  return (
    <div className="ov-inbox">
      <p className="ov-inbox-head">
        <BellIcon size={14} weight="fill" aria-hidden />
        Inbox
        <span className="ov-inbox-count">{items.length}</span>
      </p>
      {items.length ? (
        <ul className="ov-inbox-list">
          {items.map((item) => (
            <li key={item.id} className="ov-inbox-row">
              <item.icon
                size={14}
                weight="fill"
                className="ov-inbox-icon"
                aria-hidden
              />
              <span className="ov-inbox-title">{item.title}</span>
              <button
                type="button"
                className="ov-inbox-archive"
                aria-label={`Archive: ${item.title}`}
                onClick={() => archive(item)}
              >
                <ArchiveIcon size={14} weight="bold" />
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <div className="ov-inbox-zero">
          <p>All caught up</p>
          <p className="ov-inbox-zero-note">Zero inbox, zero config ▲</p>
          <button
            type="button"
            className="b-btn"
            onClick={() => setItems(inboxSeed)}
          >
            Bring them back
          </button>
        </div>
      )}
      <ToastRegion
        toaster={toaster}
        side="bottom"
        align="end"
        maxVisible={3}
        className="ov-toasts"
      >
        {() => (
          <Toast className="ov-toast">
            <div className="ov-toast-text">
              <ToastTitle className="ov-toast-title" />
              <ToastDescription className="ov-toast-desc" />
            </div>
            <ToastAction className="b-btn ov-toast-undo" />
            <ToastClose className="ov-toast-x" aria-label="Dismiss">
              <XIcon size={14} weight="bold" />
            </ToastClose>
          </Toast>
        )}
      </ToastRegion>
    </div>
  )
}

/* 17. A popup of our own: definePrimitive + createPopup */

const statusPopup = definePrimitive({
  name: "ov-status",
  options: { defaultOpen: "boolean" },
  state: (options: { defaultOpen?: boolean }) => popupState(options),
  api: (context) => createPopup(context, { preset: popoverPreset }),
  slots: (slot) => ({
    trigger: popupTrigger(slot),
    content: popupContent(slot),
    close: popupClose(slot),
  }),
})

const StatusPopup = createParts(statusPopup, {
  trigger: "button",
  content: "div",
  close: "button",
})

const presence = [
  { id: "available", label: "Available", until: "" },
  { id: "focus", label: "Focusing", until: "until 15:00" },
  { id: "meeting", label: "In a meeting", until: "for 45 min" },
  { id: "away", label: "Away", until: "back Monday" },
]

function Presence() {
  const [status, setStatus] = useState("focus")
  const current = presence.find((p) => p.id === status)!
  return (
    <div className="ov-presence">
      <div className="ov-presence-who">
        <span className="ov-presence-avatar">
          <Avatar initials="HN" size="lg" />
          <span className="ov-presence-dot" data-status={status} aria-hidden />
        </span>
        <span className="ov-presence-text">
          <span className="ov-presence-name">Hana Novak</span>
          <span className="ov-presence-role">Infra · Prague</span>
        </span>
      </div>
      <StatusPopup.Root>
        <StatusPopup.Trigger className="b-btn ov-trigger ov-presence-trigger">
          <span
            className="ov-presence-dot ov-presence-dot-inline"
            data-status={status}
            aria-hidden
          />
          {current.label}
          {current.until && (
            <span className="ov-presence-until">{current.until}</span>
          )}
          <CaretDownIcon size={14} weight="bold" aria-hidden />
        </StatusPopup.Trigger>
        <StatusPopup.Content
          className="b-pop ov-presence-pop"
          aria-label="Set status"
        >
          {presence.map((p) => (
            <StatusPopup.Close
              key={p.id}
              className="b-item ov-presence-option"
              aria-pressed={p.id === status}
              onClick={() => setStatus(p.id)}
            >
              <span
                className="ov-presence-dot ov-presence-dot-inline"
                data-status={p.id}
                aria-hidden
              />
              <span className="ov-presence-label">{p.label}</span>
              {p.until && <span className="ov-presence-until">{p.until}</span>}
            </StatusPopup.Close>
          ))}
        </StatusPopup.Content>
      </StatusPopup.Root>
    </div>
  )
}

/* 18. position + bindLayer: a comment bubble pinned to a line */

const backoffLines = [
  { n: 12, kind: "", code: "export function nextDelay(attempt: number) {" },
  { n: 13, kind: "del", code: "  return BASE_MS * 2 ** attempt" },
  { n: 14, kind: "add", code: "  const raw = BASE_MS * 2 ** attempt" },
  { n: 15, kind: "add", code: "  return Math.min(raw, MAX_MS) + jitter()" },
  { n: 16, kind: "", code: "}" },
]

function LineComments() {
  const [openLine, setOpenLine] = useState<number | null>(null)
  const [draft, setDraft] = useState("")
  const [notes, setNotes] = useState<Record<number, string>>({
    15: "Jitter fixes the thundering herd.",
  })
  const anchors = useRef(new Map<number, HTMLButtonElement>())
  const bubble = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (openLine === null) {
      return
    }
    const el = bubble.current
    const anchor = anchors.current.get(openLine)
    if (!el || !anchor) {
      return
    }
    const stopLayer = bindLayer(el, {
      topLayer: true,
      contains: (target) => anchor.contains(target),
      onDismiss: (reason) => {
        setOpenLine(null)
        if (reason === "escape-key") {
          anchor.focus()
        }
      },
    })
    const stopPosition = position(anchor, el, {
      side: "bottom",
      align: "end",
      sideOffset: 6,
      collisionPadding: 12,
    })
    el.querySelector("textarea")?.focus()
    return () => {
      stopPosition()
      stopLayer()
    }
  }, [openLine])

  function post() {
    if (openLine === null) {
      return
    }
    const line = openLine
    if (draft.trim()) {
      setNotes((all) => ({ ...all, [line]: draft.trim() }))
    }
    setOpenLine(null)
    anchors.current.get(line)?.focus()
  }

  return (
    <div className="ov-lines">
      <p className="ov-lines-file">src/queue/backoff.ts</p>
      <ol className="ov-lines-code">
        {backoffLines.map((line) => (
          <li key={line.n} data-kind={line.kind || undefined}>
            <button
              type="button"
              className="ov-line"
              ref={(el) => {
                if (el) {
                  anchors.current.set(line.n, el)
                } else {
                  anchors.current.delete(line.n)
                }
              }}
              aria-expanded={openLine === line.n}
              aria-label={`Line ${line.n}: ${line.code.trim()}. Comment`}
              data-open={openLine === line.n || undefined}
              onClick={() => {
                setDraft(notes[line.n] ?? "")
                setOpenLine(openLine === line.n ? null : line.n)
              }}
            >
              <span className="ov-line-n">{line.n}</span>
              <code className="ov-line-code">{line.code}</code>
              {notes[line.n] && (
                <ChatCircleIcon
                  size={14}
                  weight="fill"
                  className="ov-line-note"
                />
              )}
            </button>
          </li>
        ))}
      </ol>
      {openLine !== null && (
        <div
          ref={bubble}
          className="ov-bubble"
          role="dialog"
          aria-label={`Comment on line ${openLine}`}
        >
          <textarea
            className="b-input ov-bubble-input"
            rows={2}
            placeholder="Leave a comment"
            aria-label="Comment"
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                post()
              }
            }}
          />
          <div className="ov-bubble-actions">
            <button
              type="button"
              className="b-btn"
              data-variant="ghost"
              onClick={() => setOpenLine(null)}
            >
              Discard
            </button>
            <button
              type="button"
              className="b-btn"
              data-variant="primary"
              onClick={post}
            >
              Comment
            </button>
          </div>
        </div>
      )}
    </div>
  )
}

/* 19. Popover holding a Command list: an assignee picker */

const assignees = [
  { id: "amara", name: "Amara Diallo", initials: "AD" },
  { id: "kenji", name: "Kenji Sato", initials: "KS" },
  { id: "elif", name: "Elif Kaya", initials: "EK" },
  { id: "owen", name: "Owen Price", initials: "OP" },
  { id: "yusuf", name: "Yusuf Demir", initials: "YD" },
]

function Assignee() {
  const [open, setOpen] = useState(false)
  const [who, setWho] = useState<string | null>("kenji")
  const person = assignees.find((a) => a.id === who)
  return (
    <div className="ov-issue">
      <p className="ov-issue-id">
        <span className="ov-issue-prio" aria-hidden />
        ENG-482 · Urgent
      </p>
      <p className="ov-issue-title">Webhook retries drop payloads</p>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger
          className="ov-assignee"
          aria-label={`Assignee: ${person?.name ?? "nobody"}`}
        >
          {person ? (
            <Avatar initials={person.initials} size="sm" />
          ) : (
            <span className="ov-avatar ov-avatar-empty" data-size="sm" />
          )}
          {person?.name ?? "Unassigned"}
          <CaretDownIcon size={14} weight="bold" aria-hidden />
        </PopoverTrigger>
        <PopoverContent
          className="b-pop ov-assign-pop"
          side="bottom"
          align="start"
          sideOffset={6}
          collisionPadding={12}
          aria-label="Assign"
        >
          <Command
            className="ov-assign"
            defaultValue={who ?? "none"}
            onAction={(value) => {
              setWho(value === "none" ? null : value)
              setOpen(false)
            }}
          >
            <CommandInput
              aria-label="Assign to"
              placeholder="Assign to…"
              className="ov-assign-input"
            />
            <CommandEmpty className="ov-cmd-empty">
              No teammate by that name
            </CommandEmpty>
            <CommandList className="ov-assign-list">
              <CommandItem
                value="none"
                label="No assignee"
                className="b-item ov-assign-item"
              >
                <span
                  className="ov-avatar ov-avatar-empty"
                  data-size="sm"
                  aria-hidden
                />
                No assignee
              </CommandItem>
              {assignees.map((a) => (
                <CommandItem
                  key={a.id}
                  value={a.id}
                  label={a.name}
                  className="b-item ov-assign-item"
                >
                  <Avatar initials={a.initials} size="sm" />
                  {a.name}
                  {a.id === who && (
                    <CheckIcon
                      size={14}
                      weight="bold"
                      className="ov-assign-check"
                      aria-label="Current"
                    />
                  )}
                </CommandItem>
              ))}
            </CommandList>
          </Command>
        </PopoverContent>
      </Popover>
    </div>
  )
}

export const tiles: Tile[] = [
  {
    id: "overlays-palette",
    title: "Jump anywhere.",
    blurb: "Search commands, then drill into pages.",
    size: "large",
    Demo: Palette,
  },
  {
    id: "overlays-invite",
    title: "Bring people in.",
    blurb: "A dialog that waits while it sends.",
    tone: "paper",
    Demo: Invite,
  },
  {
    id: "overlays-delete",
    title: "Think twice.",
    blurb: "Confirm before anything gets deleted.",
    Demo: DeleteProject,
  },
  {
    id: "overlays-tokens",
    title: "Ask, then await.",
    blurb: "One confirm call answers every row.",
    tone: "paper",
    Demo: Tokens,
  },
  {
    id: "overlays-region",
    title: "Open from code.",
    blurb: "Await the region someone picks.",
    tone: "sky",
    Demo: Region,
  },
  {
    id: "overlays-build-log",
    title: "Pull up the log.",
    blurb: "Drag the sheet between two heights.",
    tone: "ink",
    Demo: BuildLog,
  },
  {
    id: "overlays-schedule",
    title: "Pick a time.",
    blurb: "A small form that points at its button.",
    Demo: Schedule,
  },
  {
    id: "overlays-review",
    title: "Review by keyboard.",
    blurb: "Tooltips teach the keys that work.",
    tone: "blueprint",
    Demo: DiffToolbar,
  },
  {
    id: "overlays-quick-keys",
    title: "Keys for your machine.",
    blurb: "⌘ on a Mac, Ctrl everywhere else.",
    tone: "signal",
    Demo: QuickKeys,
  },
  {
    id: "overlays-mention",
    title: "Know who’s who.",
    blurb: "Pause on a mention to meet them.",
    Demo: Mention,
  },
  {
    id: "overlays-deploys",
    title: "Every row, its actions.",
    blurb: "Menus with submenus, named by tooltips.",
    size: "wide",
    Demo: Deploys,
  },
  {
    id: "overlays-file-tree",
    title: "Right-click works.",
    blurb: "A context menu that knows the file.",
    tone: "paper",
    Demo: FileTree,
  },
  {
    id: "overlays-branch",
    title: "Menu, then dialog.",
    blurb: "Rename opens a dialog from the menu.",
    tone: "sage",
    Demo: BranchRename,
  },
  {
    id: "overlays-menubar",
    title: "A real menu bar.",
    blurb: "Arrow across menus, toggle the view.",
    size: "wide",
    tone: "paper",
    Demo: Editor,
  },
  {
    id: "overlays-sidebar",
    title: "Navigate the workspace.",
    blurb: "Sections open panels beside the sidebar.",
    size: "tall",
    Demo: Sidebar,
  },
  {
    id: "overlays-inbox",
    title: "Undo anything.",
    blurb: "Archived notices stack up with Undo.",
    tone: "butter",
    Demo: Inbox,
  },
  {
    id: "overlays-presence",
    title: "Roll your own.",
    blurb: "A status popup from core popup parts.",
    Demo: Presence,
  },
  {
    id: "overlays-line-comments",
    title: "Pin it to a line.",
    blurb: "Placed and dismissed without a component.",
    tone: "paper",
    Demo: LineComments,
  },
  {
    id: "overlays-assignee",
    title: "Assign in a keystroke.",
    blurb: "Type a name, press Enter.",
    tone: "ink",
    Demo: Assignee,
  },
]
