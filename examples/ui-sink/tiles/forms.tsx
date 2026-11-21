import {
  AirplaneLandingIcon,
  AirplaneTakeoffIcon,
  ArmchairIcon,
  ArrowRightIcon,
  ArrowsLeftRightIcon,
  BankIcon,
  BarbellIcon,
  BedIcon,
  CaretDownIcon,
  CheckCircleIcon,
  CheckIcon,
  CircleNotchIcon,
  CoffeeIcon,
  CoinsIcon,
  CreditCardIcon,
  EyeIcon,
  EyeSlashIcon,
  FilePdfIcon,
  HeartIcon,
  IdentificationCardIcon,
  LockSimpleIcon,
  MagnifyingGlassIcon,
  MinusIcon,
  MoonIcon,
  PawPrintIcon,
  PlusIcon,
  SeatIcon,
  StarIcon,
  SuitcaseRollingIcon,
  SwimmingPoolIcon,
  TicketIcon,
  VanIcon,
  XIcon,
} from "@phosphor-icons/react"
import { Button } from "@ui-sdk/react/button"
import {
  Checkbox,
  CheckboxGroup,
  CheckboxIndicator,
} from "@ui-sdk/react/checkbox"
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
  ComboboxItemText,
  ComboboxLabel,
  ComboboxList,
  ComboboxValue,
  createComboboxController,
} from "@ui-sdk/react/combobox"
import {
  Field,
  FieldDescription,
  FieldError,
  FieldLabel,
} from "@ui-sdk/react/field"
import {
  FileInput,
  FileInputDropzone,
  FileInputItem,
  FileInputItemRemove,
  FileInputList,
  FileInputTrigger,
  formatFileSize,
} from "@ui-sdk/react/file-input"
import { Input, InputClear, InputGroup } from "@ui-sdk/react/input"
import {
  NumberInput,
  NumberInputControl,
  NumberInputDecrement,
  NumberInputGroup,
  NumberInputIncrement,
} from "@ui-sdk/react/number-input"
import {
  OTPInput,
  OTPInputGroup,
  OTPInputSeparator,
  OTPInputSlot,
} from "@ui-sdk/react/otp-input"
import {
  PasswordInput,
  PasswordInputControl,
  PasswordInputToggle,
} from "@ui-sdk/react/password-input"
import { Radio, RadioGroup, RadioIndicator } from "@ui-sdk/react/radio"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectItemIcon,
  SelectItemIndicator,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@ui-sdk/react/select"
import {
  Slider,
  SliderControl,
  SliderIndicator,
  SliderThumb,
  SliderTrack,
  SliderValue,
} from "@ui-sdk/react/slider"
import { Switch, SwitchThumb } from "@ui-sdk/react/switch"
import { Textarea } from "@ui-sdk/react/textarea"
import { Toggle, ToggleGroup } from "@ui-sdk/react/toggle"
import { useState, type ReactNode } from "react"
import {
  SegmentedControl,
  SegmentedControlIndicator,
  SegmentedControlItem,
} from "@ui-sdk/react/segmented-control"
import type { Tile } from "../bento/types"
import "./forms.css"

const wait = (ms: number) =>
  new Promise<void>((resolve) => setTimeout(resolve, ms))

/** A press handler for a submit Button: pending only when the form is valid, so invalid forms still show their errors. */
const submitWhenValid = (
  event: { currentTarget: EventTarget },
  work: () => Promise<void>
) => {
  const form = (event.currentTarget as HTMLElement).closest("form")
  if (form && !form.checkValidity()) {
    return
  }
  return work()
}

function Spinner({ size = 16 }: { size?: number }) {
  return (
    <CircleNotchIcon
      className="fm-spin"
      size={size}
      weight="bold"
      aria-hidden
    />
  )
}

/* 1. Checkout card form */

const countries = [
  { value: "pt", label: "Portugal" },
  { value: "jp", label: "Japan" },
  { value: "us", label: "United States" },
  { value: "ca", label: "Canada" },
  { value: "de", label: "Germany" },
]

function Checkout() {
  const [card, setCard] = useState("")
  const [paid, setPaid] = useState(false)
  return (
    <form className="fm-pay" onSubmit={(event) => event.preventDefault()}>
      <header className="fm-pay-head">
        <div>
          <p className="fm-pay-route">
            Lisbon <ArrowRightIcon size={14} weight="bold" aria-hidden /> Tokyo
          </p>
          <p className="fm-pay-meta">2 travelers · Oct 9 – 23</p>
        </div>
        <p className="fm-pay-total">$1,284.60</p>
      </header>

      <Field name="cardholder" className="fm-field">
        <div className="fm-field-head">
          <FieldLabel className="b-label">Name on card</FieldLabel>
          <FieldError className="fm-error" match="valueMissing">
            Required
          </FieldError>
        </div>
        <Input
          className="b-input fm-input"
          autoComplete="cc-name"
          placeholder="Maya Okafor"
          required
        />
      </Field>

      <Field name="card" className="fm-field">
        <div className="fm-field-head">
          <FieldLabel className="b-label">Card number</FieldLabel>
          <FieldError className="fm-error" match="valueMissing">
            Required
          </FieldError>
          <FieldError className="fm-error" match="patternMismatch">
            Needs 16 digits
          </FieldError>
        </div>
        <InputGroup className="b-input fm-input-group">
          <CreditCardIcon size={14} weight="fill" aria-hidden />
          <Input
            className="fm-bare-input fm-mono"
            value={card}
            onValueChange={(value, details) => {
              const digits = value.replace(/\D/g, "")
              if (digits.length > 16) {
                return details.cancel()
              }
              setCard(digits.replace(/(\d{4})(?=\d)/g, "$1 "))
            }}
            inputMode="numeric"
            autoComplete="cc-number"
            placeholder="1234 1234 1234 1234"
            pattern="\d{4} \d{4} \d{4} \d{4}"
            required
          />
        </InputGroup>
      </Field>

      <div className="fm-pay-row">
        <Field name="expiry" className="fm-field">
          <div className="fm-field-head">
            <FieldLabel className="b-label">Expires</FieldLabel>
            <FieldError className="fm-error">Invalid</FieldError>
          </div>
          <Input
            className="b-input fm-input fm-mono"
            autoComplete="cc-exp"
            placeholder="MM / YY"
            pattern="(0[1-9]|1[0-2]) ?/ ?\d{2}"
            maxLength={7}
            required
          />
        </Field>
        <Field name="cvc" className="fm-field">
          <div className="fm-field-head">
            <FieldLabel className="b-label">CVC</FieldLabel>
            <FieldError className="fm-error">Invalid</FieldError>
          </div>
          <Input
            className="b-input fm-input fm-mono"
            autoComplete="cc-csc"
            inputMode="numeric"
            placeholder="123"
            pattern="\d{3,4}"
            maxLength={4}
            required
          />
        </Field>
        <Field name="country" className="fm-field fm-pay-country">
          <div className="fm-field-head">
            <FieldLabel className="b-label">Billing country</FieldLabel>
            <FieldError className="fm-error" match="valueMissing">
              Required
            </FieldError>
          </div>
          <Select items={countries} required placeholder="Choose">
            <SelectTrigger className="b-input fm-trigger">
              <SelectValue className="fm-trigger-value" />
              <CaretDownIcon size={14} weight="bold" aria-hidden />
            </SelectTrigger>
            <SelectContent className="b-pop fm-pop" sideOffset={6}>
              {countries.map((country) => (
                <SelectItem
                  key={country.value}
                  value={country.value}
                  className="b-item fm-option"
                >
                  {country.label}
                  <SelectItemIndicator className="fm-option-check">
                    <CheckIcon size={14} weight="bold" />
                  </SelectItemIndicator>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
      </div>

      <label className="fm-check-row">
        <Checkbox name="save-card" defaultChecked className="fm-box">
          <CheckboxIndicator className="fm-box-mark">
            <CheckIcon size={14} weight="bold" />
          </CheckboxIndicator>
        </Checkbox>
        Save card for future bookings
      </label>

      <Button
        type="submit"
        className="b-btn fm-pay-btn"
        data-variant="primary"
        data-done={paid || undefined}
        pendingAnnouncement="Processing payment"
        onClick={(event) =>
          submitWhenValid(event, async () => {
            await wait(1200)
            setPaid(true)
            setTimeout(() => setPaid(false), 2400)
          })
        }
      >
        {(state) =>
          state.pending ? (
            <>
              <Spinner size={14} /> Processing…
            </>
          ) : paid ? (
            <>
              <CheckIcon size={14} weight="bold" aria-hidden /> Payment approved
            </>
          ) : (
            <>
              <LockSimpleIcon size={14} weight="fill" aria-hidden /> Pay now
            </>
          )
        }
      </Button>
    </form>
  )
}

/* 2. Flight search: two airport comboboxes, a trip type and a search button */

const airports = [
  { code: "SFO", city: "San Francisco", country: "United States" },
  { code: "SEA", city: "Seattle", country: "United States" },
  { code: "JFK", city: "New York", country: "United States" },
  { code: "MEX", city: "Mexico City", country: "Mexico" },
  { code: "CDG", city: "Paris", country: "France" },
  { code: "AMS", city: "Amsterdam", country: "Netherlands" },
  { code: "KEF", city: "Reykjavík", country: "Iceland" },
  { code: "CPT", city: "Cape Town", country: "South Africa" },
  { code: "ICN", city: "Seoul", country: "South Korea" },
  { code: "SYD", city: "Sydney", country: "Australia" },
]
const airportLabel = (code: string | null) => {
  const airport = airports.find((item) => item.code === code)
  return airport ? `${airport.city} (${airport.code})` : ""
}

type AirportProps = {
  label: string
  icon: ReactNode
  value: string | null
  query: string
  onValueChange: (value: string | null) => void
  onQueryChange: (query: string) => void
  controller?: ReturnType<typeof createComboboxController<string>>
}

function AirportField({
  label,
  icon,
  value,
  query,
  onValueChange,
  onQueryChange,
  controller,
}: AirportProps) {
  const [box, setBox] = useState<HTMLDivElement | null>(null)
  return (
    <Combobox
      value={value}
      onValueChange={(next) => onValueChange(next as string | null)}
      query={query}
      onQueryChange={onQueryChange}
      itemToLabel={(code) => airportLabel(code as string)}
      autoHighlight
      controller={controller}
    >
      <div className="fm-ap" ref={setBox}>
        {icon}
        <div className="fm-ap-text">
          <ComboboxLabel className="fm-ap-label">{label}</ComboboxLabel>
          <ComboboxInput
            className="fm-ap-input"
            placeholder="City or airport"
          />
        </div>
      </div>
      <ComboboxContent
        anchor={box}
        sideOffset={6}
        align="start"
        className="b-pop fm-ap-pop"
      >
        <ComboboxEmpty className="fm-empty">No airports match.</ComboboxEmpty>
        <ComboboxList className="fm-list">
          {airports.map((airport) => (
            <ComboboxItem
              key={airport.code}
              value={airport.code}
              label={airportLabel(airport.code)}
              keywords={[airport.code, airport.country]}
              className="b-item fm-ap-item"
            >
              <span className="fm-ap-code">{airport.code}</span>
              <span className="fm-ap-city">
                {airport.city}
                <span>{airport.country}</span>
              </span>
            </ComboboxItem>
          ))}
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
  )
}

function FlightSearch() {
  const [trip, setTrip] = useState("round")
  const [from, setFrom] = useState<string | null>("SFO")
  const [fromQuery, setFromQuery] = useState(airportLabel("SFO"))
  const [to, setTo] = useState<string | null>(null)
  const [toQuery, setToQuery] = useState("")
  const [result, setResult] = useState("")
  const [toController] = useState(() => createComboboxController())
  const swap = () => {
    setFrom(to)
    setTo(from)
    setFromQuery(toQuery)
    setToQuery(fromQuery)
    setResult("")
  }
  return (
    <div className="fm-search">
      <SegmentedControl
        aria-label="Trip type"
        value={trip}
        onValueChange={(value) => value && setTrip(value)}
        className="fm-seg"
      >
        <SegmentedControlIndicator className="fm-seg-indicator" />
        <SegmentedControlItem value="round" className="fm-seg-item">
          Round trip
        </SegmentedControlItem>
        <SegmentedControlItem value="oneway" className="fm-seg-item">
          One way
        </SegmentedControlItem>
        <SegmentedControlItem value="multi" className="fm-seg-item">
          Multi-city
        </SegmentedControlItem>
      </SegmentedControl>
      <div className="fm-search-bar">
        <AirportField
          label="From"
          icon={<AirplaneTakeoffIcon size={16} weight="fill" aria-hidden />}
          value={from}
          query={fromQuery}
          onValueChange={(value) => {
            setFrom(value)
            setResult("")
          }}
          onQueryChange={setFromQuery}
        />
        <button
          type="button"
          className="fm-swap"
          aria-label="Swap origin and destination"
          onClick={swap}
        >
          <ArrowsLeftRightIcon size={16} weight="bold" />
        </button>
        <AirportField
          label="To"
          icon={<AirplaneLandingIcon size={16} weight="fill" aria-hidden />}
          value={to}
          query={toQuery}
          onValueChange={(value) => {
            setTo(value)
            setResult("")
          }}
          onQueryChange={setToQuery}
          controller={toController}
        />
        <Button
          className="b-btn fm-search-btn"
          data-variant="primary"
          pendingAnnouncement="Searching flights"
          onClick={async () => {
            if (!from || !to) {
              setResult("Choose both airports first.")
              if (!to) {
                toController.focus()
              }
              return
            }
            setResult("")
            await wait(1000)
            const seed = [...from, ...to].reduce(
              (sum, char) => sum + char.charCodeAt(0),
              0
            )
            const way =
              trip === "oneway"
                ? "one-way"
                : trip === "multi"
                  ? "multi-city"
                  : "round-trip"
            setResult(
              `${(seed % 40) + 14} ${way} fares from $${300 + ((seed * 7) % 520)}`
            )
          }}
        >
          {(state) =>
            state.pending ? (
              <>
                <Spinner /> Searching
              </>
            ) : (
              <>
                <MagnifyingGlassIcon size={16} weight="bold" aria-hidden />{" "}
                Search
              </>
            )
          }
        </Button>
      </div>
      <p className="fm-search-status" role="status">
        {result}
      </p>
    </div>
  )
}

/* 3. Seat map: a single-select toggle group laid out like a cabin */

const seatRows = Array.from({ length: 14 }, (_, index) => index + 11)
const seatLetters = ["A", "B", "C", "D", "E", "F"]
const takenSeats = new Set(
  "11A 11B 12C 12D 13B 13F 14A 14B 15E 16F 17C 17D 17E 18A 18F 19B 19F 20A 20C 21D 21E 22A 22F 23B 23C 24D 24E".split(
    " "
  )
)
const legroomRows = new Set([11, 16])
const seatInfo = (id: string) => {
  const row = Number(id.slice(0, -1))
  const letter = id.slice(-1)
  const kind = "AF".includes(letter)
    ? "Window"
    : "CD".includes(letter)
      ? "Aisle"
      : "Middle"
  const price = legroomRows.has(row) ? 46 : kind === "Middle" ? 18 : 24
  return { kind, price, legroom: legroomRows.has(row) }
}

function SeatMap() {
  const [seat, setSeat] = useState<string[]>(["16C"])
  const [held, setHeld] = useState("")
  const chosen = seat[0]
  const info = chosen ? seatInfo(chosen) : null
  return (
    <div className="fm-cabin-map">
      <div className="fm-plane">
        <ToggleGroup
          aria-label="Seats, rows 11 to 24"
          value={seat}
          onValueChange={(value) => {
            setSeat(value)
            setHeld("")
          }}
          className="fm-seats"
        >
          {seatLetters.map((letter, line) => (
            <span
              key={letter}
              aria-hidden
              className="fm-seat-letter"
              style={{ gridRow: line < 3 ? line + 1 : line + 2 }}
            >
              {letter}
            </span>
          ))}
          {seatRows.map((row) => (
            <span
              key={row}
              aria-hidden
              className="fm-seat-row"
              data-far={row > 16 || undefined}
              style={{ gridColumn: row - 9, gridRow: 4 }}
            >
              {row}
            </span>
          ))}
          {seatLetters.flatMap((letter, line) =>
            seatRows.map((row) => {
              const id = `${row}${letter}`
              const taken = takenSeats.has(id)
              return (
                <Toggle
                  key={id}
                  value={id}
                  disabled={taken}
                  aria-label={`Seat ${id}, ${seatInfo(id).kind.toLowerCase()}${taken ? ", taken" : ""}`}
                  className="fm-seat"
                  data-legroom={legroomRows.has(row) || undefined}
                  data-far={row > 16 || undefined}
                  style={{
                    gridColumn: row - 9,
                    gridRow: line < 3 ? line + 1 : line + 2,
                  }}
                />
              )
            })
          )}
        </ToggleGroup>
      </div>
      <div className="fm-seat-panel">
        <p className="fm-seat-id">{chosen ?? "—"}</p>
        <p className="fm-seat-kind">
          {info
            ? `${info.kind}${info.legroom ? " · extra legroom" : ""}`
            : "Pick an open seat"}
        </p>
        <p className="fm-seat-price">{info ? `$${info.price}` : ""}</p>
        <Button
          className="b-btn fm-seat-btn"
          data-variant="primary"
          disabled={!chosen}
          onClick={async () => {
            await wait(900)
            setHeld(chosen ?? "")
          }}
        >
          {(state) =>
            state.pending ? (
              <>
                <Spinner /> Holding
              </>
            ) : held && held === chosen ? (
              <>
                <CheckIcon size={16} weight="bold" aria-hidden /> Seat held
              </>
            ) : (
              "Hold seat"
            )
          }
        </Button>
      </div>
    </div>
  )
}

/* 4. Alerts: checkbox groups with a parent, a switch and an async save */

type Option = { value: string; label: string }

function AlertGroup({
  title,
  items,
  initial,
}: {
  title: string
  items: Option[]
  initial: string[]
}) {
  const [value, setValue] = useState(initial)
  return (
    <CheckboxGroup
      aria-label={title}
      value={value}
      onValueChange={setValue}
      allValues={items.map((item) => item.value)}
      className="fm-al-group"
    >
      <label className="fm-al-parent">
        <Checkbox parent className="fm-box">
          <CheckboxIndicator
            className="fm-box-mark"
            render={(props, state) => (
              <span {...props}>
                {state.indeterminate ? (
                  <MinusIcon size={14} weight="bold" />
                ) : (
                  <CheckIcon size={14} weight="bold" />
                )}
              </span>
            )}
          />
        </Checkbox>
        <span className="fm-al-title">{title}</span>
        <span className="fm-al-count">
          {value.length} of {items.length}
        </span>
      </label>
      <div className="fm-al-children">
        {items.map((item) => (
          <label key={item.value} className="fm-al-row">
            <Checkbox value={item.value} className="fm-box">
              <CheckboxIndicator className="fm-box-mark">
                <CheckIcon size={14} weight="bold" />
              </CheckboxIndicator>
            </Checkbox>
            {item.label}
          </label>
        ))}
      </div>
    </CheckboxGroup>
  )
}

function Alerts() {
  const [quiet, setQuiet] = useState(true)
  const [saved, setSaved] = useState(false)
  return (
    <div className="fm-alerts">
      <div className="fm-alerts-head">
        <p className="fm-alerts-title">Notifications</p>
        <p className="fm-alerts-sub">Sent to your phone and inbox</p>
      </div>
      <AlertGroup
        title="Trip updates"
        initial={["gate", "delay", "boarding"]}
        items={[
          { value: "gate", label: "Gate changes" },
          { value: "delay", label: "Delays over 30 min" },
          { value: "boarding", label: "Boarding calls" },
          { value: "belt", label: "Baggage belt" },
        ]}
      />
      <AlertGroup
        title="Price drops"
        initial={[]}
        items={[
          { value: "routes", label: "Watched routes" },
          { value: "stays", label: "Hotels on your list" },
        ]}
      />
      <label className="fm-quiet">
        <span className="fm-quiet-text">
          <span>Quiet hours</span>
          <span className="fm-quiet-detail">Hold alerts 10 pm – 7 am</span>
        </span>
        <Switch
          checked={quiet}
          onCheckedChange={(next) => {
            setQuiet(next)
            setSaved(false)
          }}
          className="fm-sq-switch"
        >
          <SwitchThumb className="fm-sq-thumb">
            <MoonIcon size={14} weight="fill" aria-hidden />
          </SwitchThumb>
        </Switch>
      </label>
      <Button
        className="b-btn fm-alerts-save"
        data-variant="primary"
        onClick={async () => {
          await wait(900)
          setSaved(true)
        }}
      >
        {(state) =>
          state.pending ? (
            <>
              <Spinner size={14} /> Saving
            </>
          ) : saved ? (
            <>
              <CheckIcon size={14} weight="bold" aria-hidden /> Preferences
              saved
            </>
          ) : (
            "Save preferences"
          )
        }
      </Button>
    </div>
  )
}

/* 5. Cabin: card radios */

const cabins = [
  { value: "economy", label: "Economy", price: "$412", icon: SeatIcon },
  { value: "premium", label: "Premium", price: "$689", icon: ArmchairIcon },
  { value: "business", label: "Business", price: "$1,940", icon: BedIcon },
]

function Cabin() {
  return (
    <RadioGroup defaultValue="premium" aria-label="Cabin" className="fm-cabins">
      {cabins.map(({ value, label, price, icon: Icon }) => (
        <label key={value} className="fm-cabin">
          <span className="fm-cabin-top">
            <Icon size={16} weight="fill" aria-hidden />
            <Radio value={value} className="fm-dot">
              <RadioIndicator className="fm-dot-in" />
            </Radio>
          </span>
          <span className="fm-cabin-name">{label}</span>
          <span className="fm-cabin-price">{price}</span>
        </label>
      ))}
    </RadioGroup>
  )
}

/* 6. Verify: OTP with auto-check on complete and an async resend */

function Verify() {
  const [status, setStatus] = useState<"idle" | "checking" | "ok">("idle")
  return (
    <div className="fm-otp">
      <p className="fm-otp-lead">Code sent to +1 415 ••• 0192</p>
      <OTPInput
        maxLength={6}
        aria-label="Verification code"
        className="fm-otp-root"
        onValueChange={() => setStatus("idle")}
        onValueCommit={() => {
          setStatus("checking")
          setTimeout(
            () =>
              setStatus((current) => (current === "checking" ? "ok" : current)),
            900
          )
        }}
      >
        <OTPInputGroup className="fm-otp-group">
          {[0, 1, 2].map((index) => (
            <OTPInputSlot key={index} index={index} className="fm-slot" />
          ))}
        </OTPInputGroup>
        <OTPInputSeparator className="fm-otp-sep" />
        <OTPInputGroup className="fm-otp-group">
          {[3, 4, 5].map((index) => (
            <OTPInputSlot key={index} index={index} className="fm-slot" />
          ))}
        </OTPInputGroup>
      </OTPInput>
      <div className="fm-otp-foot">
        <p className="fm-otp-status" role="status" data-state={status}>
          {status === "checking" ? (
            <>
              <Spinner size={14} /> Checking
            </>
          ) : status === "ok" ? (
            <>
              <CheckCircleIcon size={14} weight="fill" aria-hidden /> Verified
            </>
          ) : (
            "Didn’t get it?"
          )}
        </p>
        <Button
          className="b-btn fm-otp-resend"
          data-variant="ghost"
          onClick={() => wait(900)}
        >
          {(state) =>
            state.pending ? (
              <>
                <Spinner size={14} /> Sending
              </>
            ) : (
              "Resend code"
            )
          }
        </Button>
      </div>
    </div>
  )
}

/* 7. New password */

const passwordRules = [
  { label: "8+ characters", test: (value: string) => value.length >= 8 },
  { label: "A number", test: (value: string) => /\d/.test(value) },
  { label: "A symbol", test: (value: string) => /[^\w\s]/.test(value) },
]
const strengthWords = ["Too short", "Weak", "Fair", "Good", "Strong"]

function NewPassword() {
  const [password, setPassword] = useState("lagos2026")
  const passed = passwordRules.filter((rule) => rule.test(password)).length
  const score = password
    ? Math.min(4, passed + (password.length >= 12 ? 1 : 0))
    : 0
  return (
    <Field name="new-password" className="fm-pw">
      <div className="fm-field-head">
        <FieldLabel className="b-label">New password</FieldLabel>
        <span className="fm-pw-word" data-score={score}>
          {strengthWords[score]}
        </span>
      </div>
      <PasswordInput
        value={password}
        onValueChange={setPassword}
        autoComplete="new-password"
        className="fm-pw-box"
      >
        <PasswordInputControl className="fm-bare-input fm-pw-input" />
        <PasswordInputToggle aria-label="Show password" className="fm-pw-eye">
          {(state) =>
            state.visible ? (
              <EyeSlashIcon size={16} weight="bold" />
            ) : (
              <EyeIcon size={16} weight="bold" />
            )
          }
        </PasswordInputToggle>
      </PasswordInput>
      <div className="fm-pw-meter" data-score={score} aria-hidden>
        <i />
        <i />
        <i />
        <i />
      </div>
      <ul className="fm-pw-rules" aria-label="Password rules">
        {passwordRules.map((rule) => {
          const met = rule.test(password)
          return (
            <li key={rule.label} data-met={met || undefined}>
              {met ? (
                <CheckIcon size={16} weight="bold" aria-hidden />
              ) : (
                <span className="fm-pw-dot" aria-hidden />
              )}
              <span className="fm-sr">{met ? "Done: " : "To do: "}</span>
              {rule.label}
            </li>
          )
        })}
      </ul>
    </Field>
  )
}

/* 8. Nights: a big stepper */

const checkIn = new Date(2026, 7, 14)
const shortDate = (date: Date) =>
  date.toLocaleDateString("en-US", {
    weekday: "short",
    month: "short",
    day: "numeric",
  })

function Nights() {
  const [nights, setNights] = useState<number | null>(3)
  const checkOut = new Date(checkIn)
  checkOut.setDate(checkIn.getDate() + (nights ?? 0))
  return (
    <div className="fm-nights">
      <NumberInput
        value={nights}
        onValueChange={setNights}
        min={1}
        max={21}
        className="fm-nights-root"
      >
        <NumberInputGroup className="fm-nights-group">
          <NumberInputDecrement
            aria-label="One night fewer"
            className="fm-round"
          >
            <MinusIcon size={16} weight="bold" />
          </NumberInputDecrement>
          <span className="fm-nights-num">
            <NumberInputControl
              aria-label="Nights"
              className="fm-nights-input"
            />
            <span className="fm-nights-unit">
              {nights === 1 ? "night" : "nights"}
            </span>
          </span>
          <NumberInputIncrement
            aria-label="One more night"
            className="fm-round"
          >
            <PlusIcon size={16} weight="bold" />
          </NumberInputIncrement>
        </NumberInputGroup>
      </NumberInput>
      <p className="fm-nights-dates">
        {shortDate(checkIn)} → {nights ? shortDate(checkOut) : "…"}
      </p>
    </div>
  )
}

/* 9. Fare range: a two-thumb slider over a histogram */

const fareBuckets = [
  3, 6, 11, 18, 26, 31, 28, 34, 30, 24, 21, 17, 19, 14, 11, 9, 8, 6, 7, 5, 4, 3,
  4, 2, 2, 1, 1, 1,
]
const fareMin = 100
const fareStep = 50

function FareRange() {
  const [range, setRange] = useState([300, 850])
  const inRange = (index: number) => {
    const price = fareMin + index * fareStep
    return price >= range[0] && price < range[1]
  }
  const shown = fareBuckets.reduce(
    (sum, count, index) => sum + (inRange(index) ? count : 0),
    0
  )
  const total = fareBuckets.reduce((sum, count) => sum + count, 0)
  const peak = Math.max(...fareBuckets)
  return (
    <Field className="fm-fare">
      <Slider
        value={range}
        onValueChange={(value) => setRange(value as number[])}
        min={fareMin}
        max={fareMin + fareBuckets.length * fareStep}
        step={fareStep}
        minStepsBetweenThumbs={2}
        format={{
          style: "currency",
          currency: "USD",
          maximumFractionDigits: 0,
        }}
        locale="en-US"
        className="fm-fare-slider"
      >
        <div className="fm-field-head">
          <FieldLabel className="b-label">Fare per person</FieldLabel>
          <SliderValue className="fm-fare-value" />
        </div>
        <div className="fm-hist" aria-hidden>
          {fareBuckets.map((count, index) => (
            <i
              key={index}
              style={{ height: `${(count / peak) * 100}%` }}
              data-in={inRange(index) || undefined}
            />
          ))}
        </div>
        <SliderControl className="fm-fare-control">
          <SliderTrack className="fm-fare-track">
            <SliderIndicator className="fm-fare-fill" />
            <SliderThumb
              index={0}
              aria-label="Lowest fare"
              className="fm-fare-thumb"
            />
            <SliderThumb
              index={1}
              aria-label="Highest fare"
              className="fm-fare-thumb"
            />
          </SliderTrack>
        </SliderControl>
      </Slider>
      <FieldDescription className="fm-fare-count">
        {shown} of {total} flights in range
      </FieldDescription>
    </Field>
  )
}

/* 10. Requests: a growing textarea with a counter and quick adds */

const requestChips = ["Late check-in", "Quiet room", "Extra pillows"]

function Requests() {
  const [note, setNote] = useState("Arriving on the 23:40 train from Milan.")
  return (
    <Field name="requests" className="fm-req">
      <div className="fm-field-head">
        <FieldLabel className="b-label">Special requests</FieldLabel>
        <FieldDescription
          className="fm-req-count"
          data-full={note.length >= 180 || undefined}
        >
          {note.length}/200
        </FieldDescription>
      </div>
      <Textarea
        value={note}
        onValueChange={setNote}
        maxLength={200}
        autoResize
        placeholder="Anything that helps the hotel prepare"
        className="b-input fm-req-area"
      />
      <div className="fm-req-chips">
        {requestChips.map((chip) => {
          const added = note.includes(chip)
          return (
            <button
              key={chip}
              type="button"
              className="fm-chip"
              disabled={added}
              onClick={() =>
                setNote((current) =>
                  `${current.trim()} ${chip}.`.trim().slice(0, 200)
                )
              }
            >
              {added ? (
                <CheckIcon size={14} weight="bold" aria-hidden />
              ) : (
                <PlusIcon size={14} weight="bold" aria-hidden />
              )}
              {chip}
            </button>
          )
        })}
      </div>
    </Field>
  )
}

/* 11. Passport upload */

function Passport() {
  return (
    <Field className="fm-doc">
      <FieldLabel className="b-label">Passport photo page</FieldLabel>
      <FileInput
        name="passport"
        accept={["image/*", ".pdf"]}
        maxSize={10_000_000}
        className="fm-doc-root"
      >
        {({ value }) =>
          value.length ? (
            <FileInputList aria-label="Passport scan" className="fm-doc-list">
              <FileInputItem className="fm-doc-file">
                {({ file }) => (
                  <>
                    <span className="fm-doc-icon">
                      <FilePdfIcon size={16} weight="fill" aria-hidden />
                    </span>
                    <span className="fm-doc-name">
                      {file.name}
                      <span>{formatFileSize(file.size)} · ready to send</span>
                    </span>
                    <FileInputItemRemove className="fm-icon-btn">
                      <XIcon size={16} weight="bold" aria-hidden />
                    </FileInputItemRemove>
                  </>
                )}
              </FileInputItem>
            </FileInputList>
          ) : (
            <FileInputDropzone className="fm-doc-drop">
              <span className="fm-doc-glyph">
                <IdentificationCardIcon size={16} weight="fill" aria-hidden />
              </span>
              <span className="fm-doc-hint">Drop a scan here</span>
              <FileInputTrigger className="b-btn fm-doc-browse">
                Browse
              </FileInputTrigger>
            </FileInputDropzone>
          )
        }
      </FileInput>
      <FieldDescription className="fm-doc-desc">
        JPG, PNG or PDF, up to 10 MB.
      </FieldDescription>
    </Field>
  )
}

/* 12. Amenities: a multiple select */

const amenities = [
  {
    value: "breakfast",
    label: "Breakfast included",
    icon: CoffeeIcon,
    share: 0.58,
  },
  { value: "pool", label: "Pool", icon: SwimmingPoolIcon, share: 0.34 },
  { value: "gym", label: "Gym", icon: BarbellIcon, share: 0.61 },
  { value: "pets", label: "Pets welcome", icon: PawPrintIcon, share: 0.27 },
  { value: "shuttle", label: "Airport shuttle", icon: VanIcon, share: 0.22 },
]

function Amenities() {
  const [value, setValue] = useState<string[]>(["breakfast", "pool"])
  const count = Math.round(
    amenities.reduce(
      (total, item) =>
        value.includes(item.value) ? total * item.share : total,
      640
    )
  )
  return (
    <div className="fm-am">
      <Select
        multiple
        items={amenities.map(({ value, label }) => ({ value, label }))}
        value={value}
        onValueChange={setValue}
        placeholder="Any amenities"
      >
        <SelectLabel className="b-label">Hotel amenities</SelectLabel>
        <SelectTrigger className="fm-am-trigger">
          <SelectValue className="fm-trigger-value">
            {(state) =>
              state.placeholder
                ? "Any amenities"
                : state.labels.length > 2
                  ? `${state.labels.slice(0, 2).join(", ")} +${state.labels.length - 2}`
                  : state.label
            }
          </SelectValue>
          <CaretDownIcon size={16} weight="bold" aria-hidden />
        </SelectTrigger>
        <SelectContent className="b-pop fm-pop" sideOffset={6}>
          {amenities.map(({ value: item, label, icon: Icon }) => (
            <SelectItem key={item} value={item} className="b-item fm-option">
              <SelectItemIcon className="fm-option-icon">
                <Icon size={16} weight="fill" />
              </SelectItemIcon>
              {label}
              <SelectItemIndicator className="fm-option-check">
                <CheckIcon size={16} weight="bold" />
              </SelectItemIndicator>
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <p className="fm-am-count">
        <strong>{count}</strong> stays in Kyoto match
      </p>
    </div>
  )
}

/* 13. Save a hotel: a lone toggle */

function SaveHotel() {
  const [saved, setSaved] = useState(false)
  return (
    <article className="fm-hotel">
      <div className="fm-hotel-top">
        <div>
          <p className="fm-hotel-name">Casa Albergaria Dona Sofia</p>
          <p className="fm-hotel-place">
            Porto, Ribeira · <StarIcon size={16} weight="fill" aria-hidden />{" "}
            4.8
          </p>
        </div>
        <Toggle
          aria-label="Save hotel"
          pressed={saved}
          onPressedChange={setSaved}
          className="fm-heart"
          render={(props, state) => (
            <button {...props}>
              <HeartIcon size={16} weight={state.pressed ? "fill" : "bold"} />
            </button>
          )}
        />
      </div>
      <div className="fm-hotel-foot">
        <p className="fm-hotel-price">
          €186<span> / night</span>
        </p>
        <p className="fm-hotel-status" role="status">
          {saved ? "Saved to Summer in Portugal" : ""}
        </p>
      </div>
    </article>
  )
}

/* 14. Name on ticket: a native form with submit and reset */

function TicketName() {
  const [status, setStatus] = useState("")
  return (
    <form
      className="fm-stub"
      onSubmit={(event) => {
        event.preventDefault()
        setStatus("Saved")
      }}
      onReset={() => setStatus("")}
    >
      <Field name="ticket-name" className="fm-field">
        <div className="fm-field-head">
          <FieldLabel className="b-label">Name on ticket</FieldLabel>
          <FieldError className="fm-error" match="valueMissing">
            Required
          </FieldError>
          <FieldError className="fm-error" match="patternMismatch">
            Letters only
          </FieldError>
        </div>
        <Input
          className="b-input fm-stub-input"
          defaultValue="AMARA J NWOSU"
          pattern="[A-Za-z ]+"
          autoComplete="name"
          spellCheck={false}
          required
          onValueChange={() => setStatus("")}
        />
        <FieldDescription className="fm-stub-desc">
          Exactly as printed in your passport.
        </FieldDescription>
      </Field>
      <div className="fm-stub-actions">
        <span className="fm-stub-status" role="status">
          {status && <CheckIcon size={14} weight="bold" aria-hidden />}
          {status}
        </span>
        <button type="reset" className="b-btn" data-variant="ghost">
          Reset
        </button>
        <button type="submit" className="b-btn" data-variant="primary">
          Save
        </button>
      </div>
    </form>
  )
}

/* 15. Promo code: Field validation from an async check */

function Promo() {
  const [code, setCode] = useState("SPRING10")
  const [error, setError] = useState<string | null>(null)
  const [applied, setApplied] = useState(false)
  return (
    <form className="fm-promo" onSubmit={(event) => event.preventDefault()}>
      <Field
        name="promo"
        invalid={error ? true : undefined}
        className="fm-field"
      >
        <div className="fm-field-head">
          <FieldLabel className="b-label">Promo code</FieldLabel>
          <FieldError className="fm-error" match="valueMissing">
            Enter a code
          </FieldError>
          {error && (
            <FieldError className="fm-error" match>
              {error}
            </FieldError>
          )}
        </div>
        <div className="fm-promo-row">
          <InputGroup className="b-input fm-input-group fm-promo-group">
            <TicketIcon size={16} weight="fill" aria-hidden />
            <Input
              className="fm-bare-input fm-mono"
              value={code}
              onValueChange={(value) => {
                setCode(value.toUpperCase())
                setError(null)
                setApplied(false)
              }}
              autoComplete="off"
              spellCheck={false}
              placeholder="CODE"
              required
            />
            <InputClear aria-label="Clear code" className="fm-icon-btn">
              <XIcon size={16} weight="bold" aria-hidden />
            </InputClear>
          </InputGroup>
          <Button
            type="submit"
            className="b-btn fm-promo-btn"
            data-variant="primary"
            pendingAnnouncement="Checking code"
            onClick={(event) =>
              submitWhenValid(event, async () => {
                await wait(900)
                if (code === "WANDER15") {
                  setApplied(true)
                } else {
                  setError(
                    code === "SPRING10" ? "Expired Apr 30" : "Not a valid code"
                  )
                }
              })
            }
          >
            {(state) => (state.pending ? <Spinner /> : "Apply")}
          </Button>
        </div>
      </Field>
      <p className="fm-promo-result" data-applied={applied || undefined}>
        {applied ? (
          <>
            <CheckCircleIcon size={16} weight="fill" aria-hidden /> 15% off ·
            −$63.30
          </>
        ) : (
          <>
            Try <span className="fm-mono">WANDER15</span>
          </>
        )}
      </p>
    </form>
  )
}

/* 16. Automatic check-in: a big switch in a Field */

function AutoCheckIn() {
  const [on, setOn] = useState(true)
  return (
    <Field className="fm-auto">
      <div className="fm-auto-text">
        <FieldLabel className="fm-auto-label">Automatic check-in</FieldLabel>
        <FieldDescription className="fm-auto-desc">
          {on
            ? "Boarding pass lands 24 h before takeoff."
            : "You’ll check in yourself at the desk."}
        </FieldDescription>
      </div>
      <Switch checked={on} onCheckedChange={setOn} className="fm-big-switch">
        <SwitchThumb className="fm-big-thumb">
          {on ? (
            <CheckIcon size={16} weight="bold" aria-hidden />
          ) : (
            <XIcon size={16} weight="bold" aria-hidden />
          )}
        </SwitchThumb>
      </Switch>
    </Field>
  )
}

/* 17. Room type: a single select in a Field */

const rooms = [
  { value: "king", label: "Deluxe king", note: "City view", price: 289 },
  { value: "twin", label: "Twin garden", note: "Ground floor", price: 264 },
  { value: "suite", label: "Junior suite", note: "Soaking tub", price: 412 },
  {
    value: "loft",
    label: "Family loft",
    note: "Sold out",
    price: 455,
    disabled: true,
  },
]

function RoomType() {
  const [room, setRoom] = useState<string | null>("king")
  const chosen = rooms.find((item) => item.value === room)
  return (
    <div className="fm-room">
      <Field name="room" className="fm-field">
        <FieldLabel className="b-label">Room type</FieldLabel>
        <Select
          items={rooms}
          value={room}
          onValueChange={(value) => setRoom(value as string | null)}
        >
          <SelectTrigger className="b-input fm-trigger fm-room-trigger">
            <SelectValue className="fm-trigger-value" />
            <span className="fm-room-price">
              {chosen ? `$${chosen.price}` : ""}
            </span>
            <CaretDownIcon size={16} weight="bold" aria-hidden />
          </SelectTrigger>
          <SelectContent className="b-pop fm-pop fm-room-pop" sideOffset={6}>
            {rooms.map((item) => (
              <SelectItem
                key={item.value}
                value={item.value}
                label={item.label}
                disabled={item.disabled}
                className="b-item fm-room-item"
              >
                <span className="fm-room-text">
                  {item.label}
                  <span>{item.note}</span>
                </span>
                <span className="fm-room-item-price">${item.price}</span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      <p className="fm-room-total">
        <span>2 nights</span>
        <strong>{chosen ? `$${chosen.price * 2}` : "—"}</strong>
      </p>
    </div>
  )
}

/* 18. Bags: a compact stepper and a checkbox that price as they change */

function Bags() {
  const [bags, setBags] = useState<number | null>(1)
  const [priority, setPriority] = useState(false)
  const total = (bags ?? 0) * 35 + (priority ? 18 : 0)
  return (
    <div className="fm-bags">
      <div className="fm-bags-row">
        <span className="fm-bags-icon">
          <SuitcaseRollingIcon size={16} weight="fill" aria-hidden />
        </span>
        <span className="fm-bags-text">
          Checked bags
          <span>23 kg each · $35</span>
        </span>
        <NumberInput
          value={bags}
          onValueChange={setBags}
          min={0}
          max={3}
          className="fm-mini"
        >
          <NumberInputGroup className="fm-mini-group">
            <NumberInputDecrement
              aria-label="Remove a bag"
              className="fm-mini-btn"
            >
              <MinusIcon size={16} weight="bold" />
            </NumberInputDecrement>
            <NumberInputControl
              aria-label="Checked bags"
              className="fm-mini-input"
            />
            <NumberInputIncrement
              aria-label="Add a bag"
              className="fm-mini-btn"
            >
              <PlusIcon size={16} weight="bold" />
            </NumberInputIncrement>
          </NumberInputGroup>
        </NumberInput>
      </div>
      <label className="fm-bags-row fm-bags-check">
        <Checkbox
          checked={priority}
          onCheckedChange={(next) => setPriority(next === true)}
          className="fm-box"
        >
          <CheckboxIndicator className="fm-box-mark">
            <CheckIcon size={16} weight="bold" />
          </CheckboxIndicator>
        </Checkbox>
        <span className="fm-bags-text">Priority boarding</span>
        <span className="fm-bags-add">+$18</span>
      </label>
      <p className="fm-bags-total">
        <span>Extras</span>
        <strong>${total}</strong>
      </p>
    </div>
  )
}

/* 19. Payment method: a required radio group in a Field */

const methods = [
  {
    value: "card",
    label: "Card ending 4417",
    detail: "Exp 08/28",
    icon: CreditCardIcon,
  },
  {
    value: "credit",
    label: "Travel credit",
    detail: "$120 left",
    icon: CoinsIcon,
  },
  { value: "bank", label: "Bank transfer", detail: "2–3 days", icon: BankIcon },
]

function PaymentMethod() {
  const [done, setDone] = useState(false)
  return (
    <form className="fm-pm" onSubmit={(event) => event.preventDefault()}>
      <Field name="method" className="fm-field">
        <FieldLabel className="b-label">Pay with</FieldLabel>
        <RadioGroup
          required
          onValueChange={() => setDone(false)}
          className="fm-pm-list"
        >
          {methods.map(({ value, label, detail, icon: Icon }) => (
            <label key={value} className="fm-pm-row">
              <Icon size={14} weight="fill" aria-hidden />
              <span className="fm-pm-label">{label}</span>
              <span className="fm-pm-detail">{detail}</span>
              <Radio value={value} className="fm-dot">
                <RadioIndicator className="fm-dot-in" />
              </Radio>
            </label>
          ))}
        </RadioGroup>
        <div className="fm-pm-foot">
          <FieldError className="fm-error" match="valueMissing">
            Choose how to pay
          </FieldError>
          <Button
            type="submit"
            className="b-btn fm-pm-btn"
            data-variant="primary"
            onClick={(event) =>
              submitWhenValid(event, async () => {
                await wait(900)
                setDone(true)
              })
            }
          >
            {(state) =>
              state.pending ? (
                <Spinner size={14} />
              ) : done ? (
                <>
                  <CheckIcon size={14} weight="bold" aria-hidden /> Confirmed
                </>
              ) : (
                "Continue"
              )
            }
          </Button>
        </div>
      </Field>
    </form>
  )
}

/* 20. Preferred airlines: a multiple combobox with chips */

const airlines = [
  { value: "aer-lingus", label: "Aer Lingus" },
  { value: "air-nz", label: "Air New Zealand" },
  { value: "alaska", label: "Alaska Airlines" },
  { value: "emirates", label: "Emirates" },
  { value: "finnair", label: "Finnair" },
  { value: "klm", label: "KLM" },
  { value: "qantas", label: "Qantas" },
  { value: "singapore", label: "Singapore Airlines" },
  { value: "turkish", label: "Turkish Airlines" },
]
const airlineLabel = (value: string) =>
  airlines.find((item) => item.value === value)?.label ?? value

function Airlines() {
  const [value, setValue] = useState<string[]>(["finnair", "qantas"])
  return (
    <div className="fm-air">
      <Combobox
        multiple
        value={value}
        onValueChange={(next) => setValue(next as string[])}
        itemToLabel={airlineLabel}
        maxSelected={4}
      >
        <div className="fm-field-head">
          <ComboboxLabel className="b-label">Preferred airlines</ComboboxLabel>
          <span className="fm-air-count">{value.length} of 4</span>
        </div>
        <ComboboxChips className="b-input fm-chips">
          <ComboboxValue>
            {(state) =>
              (state.value as string[]).map((item, index) => (
                <ComboboxChip key={item} value={item} className="fm-air-chip">
                  {state.labels[index]}
                  <ComboboxChipRemove
                    aria-label={`Remove ${state.labels[index]}`}
                    className="fm-air-remove"
                  >
                    <XIcon size={14} weight="bold" aria-hidden />
                  </ComboboxChipRemove>
                </ComboboxChip>
              ))
            }
          </ComboboxValue>
          <ComboboxInput
            className="fm-chips-input"
            placeholder={value.length >= 4 ? "" : "Add an airline"}
          />
        </ComboboxChips>
        <ComboboxContent className="b-pop fm-pop" sideOffset={6}>
          <ComboboxEmpty className="fm-empty">
            No airline by that name.
          </ComboboxEmpty>
          <ComboboxList className="fm-list">
            {airlines.map((airline) => (
              <ComboboxItem
                key={airline.value}
                value={airline.value}
                label={airline.label}
                className="b-item fm-option"
              >
                <ComboboxItemText className="fm-match" />
                <ComboboxItemIndicator className="fm-option-check">
                  <CheckIcon size={14} weight="bold" aria-hidden />
                </ComboboxItemIndicator>
              </ComboboxItem>
            ))}
          </ComboboxList>
        </ComboboxContent>
      </Combobox>
      <p className="fm-air-note">Their fares show first in results.</p>
    </div>
  )
}

export const tiles: Tile[] = [
  {
    id: "forms-checkout",
    title: "Pay for the trip.",
    blurb: "Every field checks itself before you pay.",
    size: "large",
    Demo: Checkout,
  },
  {
    id: "forms-cabin",
    title: "Choose your cabin.",
    blurb: "Whole cards select, arrows included.",
    Demo: Cabin,
  },
  {
    id: "forms-verify",
    title: "Prove it’s you.",
    blurb: "Paste or type; it checks itself.",
    tone: "sky",
    Demo: Verify,
  },
  {
    id: "forms-flight-search",
    title: "Plan the route.",
    blurb: "Type a city or a code to find airports.",
    size: "wide",
    tone: "paper",
    Demo: FlightSearch,
  },
  {
    id: "forms-alerts",
    title: "Choose your alerts.",
    blurb: "One parent box checks its whole group.",
    size: "tall",
    Demo: Alerts,
  },
  {
    id: "forms-new-password",
    title: "Make it strong.",
    blurb: "Rules tick off as you type.",
    tone: "paper",
    Demo: NewPassword,
  },
  {
    id: "forms-nights",
    title: "Stay a little longer.",
    blurb: "A stepper that clamps to your stay.",
    tone: "butter",
    Demo: Nights,
  },
  {
    id: "forms-fare-range",
    title: "Set a price window.",
    blurb: "Two thumbs frame the fares you want.",
    tone: "ink",
    Demo: FareRange,
  },
  {
    id: "forms-seat-map",
    title: "Pick a seat.",
    blurb: "Arrow through the cabin, press to choose.",
    size: "wide",
    Demo: SeatMap,
  },
  {
    id: "forms-requests",
    title: "Note for the hotel.",
    blurb: "Grows with your text and counts it.",
    Demo: Requests,
  },
  {
    id: "forms-passport",
    title: "Add your passport.",
    blurb: "Drop a scan or browse for one.",
    Demo: Passport,
  },
  {
    id: "forms-amenities",
    title: "Must-haves only.",
    blurb: "Pick several; the trigger sums them up.",
    tone: "sage",
    Demo: Amenities,
  },
  {
    id: "forms-save-hotel",
    title: "Save it for later.",
    blurb: "A pressed state that stays put.",
    tone: "paper",
    Demo: SaveHotel,
  },
  {
    id: "forms-ticket-name",
    title: "Match your passport.",
    blurb: "Reset brings back the saved name.",
    Demo: TicketName,
  },
  {
    id: "forms-promo",
    title: "Apply a promo code.",
    blurb: "Bad codes explain themselves inline.",
    tone: "blueprint",
    Demo: Promo,
  },
  {
    id: "forms-auto-check-in",
    title: "Check in on time.",
    blurb: "Flip it by tap, label or Space.",
    tone: "signal",
    Demo: AutoCheckIn,
  },
  {
    id: "forms-room-type",
    title: "Choose a room.",
    blurb: "Prices ride along in the list.",
    tone: "paper",
    Demo: RoomType,
  },
  {
    id: "forms-bags",
    title: "Add checked bags.",
    blurb: "Extras price themselves as you count.",
    Demo: Bags,
  },
  {
    id: "forms-payment-method",
    title: "Pick how to pay.",
    blurb: "Skipping a required choice gets flagged.",
    tone: "paper",
    Demo: PaymentMethod,
  },
  {
    id: "forms-airlines",
    title: "Fly who you like.",
    blurb: "Type to add airlines as chips.",
    Demo: Airlines,
  },
]
