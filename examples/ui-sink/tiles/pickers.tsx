import {
  ArrowClockwiseIcon,
  CheckIcon,
  MagicWandIcon,
  StarIcon,
  VanIcon,
  XIcon,
} from "@phosphor-icons/react"
import {
  contrastRatio,
  formatColor,
  isInGamut,
  mixColor,
  parseColor,
  toGamut,
  type Color,
} from "@ui-sdk/core/color"
import { Button } from "@ui-sdk/react/button"
import {
  ColorPicker,
  ColorPickerArea,
  ColorPickerAreaThumb,
  ColorPickerInput,
  ColorPickerSlider,
  ColorPickerSliderThumb,
  ColorPickerSwatch,
  ColorPickerSwatches,
  ColorPickerSwatchItem,
} from "@ui-sdk/react/color-picker"
import { DateInput } from "@ui-sdk/react/date-input"
import { Field, FieldLabel } from "@ui-sdk/react/field"
import { cropImage } from "@ui-sdk/react/image"
import {
  ImageCropper,
  ImageCropperImage,
  ImageCropperRotate,
  ImageCropperSelection,
  ImageCropperViewport,
  ImageCropperZoom,
  type ImageCrop,
} from "@ui-sdk/react/image-cropper"
import { Input } from "@ui-sdk/react/input"
import { Label } from "@ui-sdk/react/label"
import { Rating, RatingItem } from "@ui-sdk/react/rating"
import { TimeInput } from "@ui-sdk/react/time-input"
import { useEffect, useId, useState, type CSSProperties } from "react"
import type { Tile } from "../bento/types"
import "./pickers.css"

const white = parseColor("#fff")!
const black = parseColor("#000")!

const readableInk = (color: Color) =>
  contrastRatio(color, white) >= contrastRatio(color, black) ? white : black

/* 1. Brand color */

const brandPresets = [
  "#e5484d",
  "#f76b15",
  "#ffc53d",
  "#30a46c",
  "#12a594",
  "#0090ff",
  "#6e56cf",
  "#d6409f",
]

function BrandColor() {
  const [value, setValue] = useState("#6e56cf")
  const brand = parseColor(value) ?? black
  const ink = formatColor(readableInk(brand), "hex")
  const tint = formatColor(mixColor(brand, white, 0.86), "hex")
  return (
    <div className="pk-brand">
      <ColorPicker
        className="pk-picker"
        aria-label="Brand color"
        format="hex"
        value={value}
        onValueChange={setValue}
      >
        <ColorPickerArea className="pk-area">
          <ColorPickerAreaThumb className="pk-thumb" />
        </ColorPickerArea>
        <ColorPickerSlider channel="hue" className="pk-slider">
          <ColorPickerSliderThumb className="pk-thumb pk-slider-thumb" />
        </ColorPickerSlider>
        <div className="pk-hex">
          <ColorPickerSwatch className="pk-swatch" />
          <ColorPickerInput className="b-input pk-hex-input" aria-label="Hex" />
        </div>
        <ColorPickerSwatches className="pk-presets" aria-label="Presets">
          {brandPresets.map((preset) => (
            <ColorPickerSwatchItem
              key={preset}
              value={preset}
              className="pk-preset"
            />
          ))}
        </ColorPickerSwatches>
      </ColorPicker>

      <div
        className="pk-site"
        style={
          {
            "--pk-brand": value,
            "--pk-brand-ink": ink,
            "--pk-brand-tint": tint,
          } as CSSProperties
        }
        aria-hidden
      >
        <div className="pk-site-bar">
          <span className="pk-site-logo" />
          <span className="pk-site-name">Fieldnote</span>
          <span className="pk-site-link">Pricing</span>
        </div>
        <span className="pk-site-badge">New · Shared notebooks</span>
        <p className="pk-site-head">Notes your whole team can find.</p>
        <div className="pk-site-actions">
          <span className="pk-site-cta">Start free</span>
          <span className="pk-site-more">Book a demo</span>
        </div>
      </div>
    </div>
  )
}

/* 2. Contrast check */

const levels = [
  { name: "AA Large", min: 3 },
  { name: "AA", min: 4.5 },
  { name: "AAA", min: 7 },
]

function liftContrast(text: Color, ground: Color, target: number) {
  const toward = readableInk(ground)
  const hexAt = (amount: number) =>
    formatColor(toGamut(mixColor(text, toward, amount)), "hex")
  let low = 0
  let high = 1
  for (let step = 0; step < 16; step++) {
    const middle = (low + high) / 2
    if (contrastRatio(parseColor(hexAt(middle))!, ground) >= target) {
      high = middle
    } else {
      low = middle
    }
  }
  return hexAt(high)
}

function ColorField({
  label,
  value,
  onValueChange,
}: {
  label: string
  value: string
  onValueChange: (value: string) => void
}) {
  const color = parseColor(value)
  return (
    <Field className="pk-color-field" invalid={!color}>
      <FieldLabel className="b-label">{label}</FieldLabel>
      <div className="b-input pk-color-input">
        <span
          className="pk-dot"
          style={{ background: color ? formatColor(color) : "transparent" }}
        />
        <Input
          className="pk-bare"
          value={value}
          onValueChange={onValueChange}
          spellCheck={false}
          autoComplete="off"
        />
      </div>
    </Field>
  )
}

function useColorText(initial: string) {
  const [text, setText] = useState(initial)
  const [color, setColor] = useState(parseColor(initial)!)
  const update = (next: string) => {
    setText(next)
    const parsed = parseColor(next)
    if (parsed) {
      setColor(parsed)
    }
  }
  return { text, color, update }
}

function ContrastCheck() {
  const fore = useColorText("#8b8d98")
  const back = useColorText("#fdfcf9")
  const ratio = contrastRatio(fore.color, back.color)
  const passesAA = ratio >= 4.5
  const isWide = !isInGamut(fore.color) || !isInGamut(back.color)

  return (
    <div className="pk-contrast">
      <div
        className="pk-sample"
        style={{
          background: formatColor(back.color),
          color: formatColor(fore.color),
        }}
      >
        <p className="pk-sample-head">Your order is on its way.</p>
        <p className="pk-sample-body">
          Track it any time from Orders. We’ll text you when it’s close.
        </p>
      </div>

      <div className="pk-score">
        <p className="pk-ratio" aria-live="polite">
          {ratio.toFixed(2)}
          <span className="pk-ratio-unit">: 1</span>
        </p>
        <ul className="pk-levels" aria-label="WCAG levels">
          {levels.map((level) => {
            const passes = ratio >= level.min
            return (
              <li
                key={level.name}
                className="pk-level"
                data-pass={passes ? "" : undefined}
              >
                {passes ? (
                  <CheckIcon size={12} weight="bold" aria-label="Passes" />
                ) : (
                  <XIcon size={12} weight="bold" aria-label="Fails" />
                )}
                {level.name}
              </li>
            )
          })}
        </ul>
      </div>

      <div className="pk-color-fields">
        <ColorField
          label="Text"
          value={fore.text}
          onValueChange={fore.update}
        />
        <ColorField
          label="Background"
          value={back.text}
          onValueChange={back.update}
        />
      </div>

      <div className="pk-contrast-foot">
        <p className="pk-note">
          {isWide
            ? "Outside sRGB; checked as given."
            : passesAA
              ? "Body text reads clearly."
              : "Too faint for body text."}
        </p>
        <Button
          className="b-btn"
          data-variant="primary"
          disabled={passesAA}
          onClick={() => fore.update(liftContrast(fore.color, back.color, 4.5))}
        >
          <MagicWandIcon size={14} weight="bold" aria-hidden />
          Fix text
        </Button>
      </div>
    </div>
  )
}

/* 3. Avatar crop */

const portrait = `data:image/svg+xml,${encodeURIComponent(
  `<svg xmlns="http://www.w3.org/2000/svg" width="720" height="480"><defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ffb38a"/><stop offset=".6" stop-color="#ff7a59"/><stop offset="1" stop-color="#c2410c"/></linearGradient><linearGradient id="h" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#3b2a20"/><stop offset="1" stop-color="#1c130e"/></linearGradient></defs><rect width="720" height="480" fill="url(#s)"/><circle cx="560" cy="120" r="48" fill="#fff4e6" opacity=".85"/><path d="M0 360 140 280l120 70 140-110 160 120 160-80v220H0z" fill="#9a3412" opacity=".55"/><path d="M250 480c10-90 60-140 110-150h0c50 10 100 60 110 150z" fill="#1e3a8a"/><rect x="335" y="270" width="50" height="70" rx="20" fill="#e8a27c"/><ellipse cx="360" cy="220" rx="62" ry="74" fill="#f2b591"/><path d="M296 214c-4-60 30-98 68-98 44 0 72 34 66 96-10-30-28-46-50-52-30 16-60 24-84 54z" fill="url(#h)"/><circle cx="338" cy="226" r="5" fill="#2a1a12"/><circle cx="384" cy="226" r="5" fill="#2a1a12"/><path d="M344 258q16 12 32 0" stroke="#9a4a2e" stroke-width="4" fill="none" stroke-linecap="round"/></svg>`
)}`

function AvatarCrop() {
  const [crop, setCrop] = useState<ImageCrop>()
  const [avatar, setAvatar] = useState<string>()
  const [isSaving, setIsSaving] = useState(false)

  useEffect(
    () => () => {
      if (avatar) {
        URL.revokeObjectURL(avatar)
      }
    },
    [avatar]
  )

  const save = async () => {
    if (!crop) {
      return
    }
    setIsSaving(true)
    const file = await cropImage(portrait, crop, {
      width: 160,
      shape: "circle",
    })
    setAvatar(URL.createObjectURL(file))
    setIsSaving(false)
  }

  return (
    <ImageCropper
      className="pk-cropper"
      aria-label="Profile photo"
      src={portrait}
      mode="frame"
      aspectRatio={1}
      shape="circle"
      maxZoom={4}
      crop={crop}
      onCropChange={setCrop}
    >
      <ImageCropperViewport className="pk-viewport">
        <ImageCropperImage className="pk-image" alt="Portrait at sunset" />
        <ImageCropperSelection className="pk-selection" />
      </ImageCropperViewport>
      <div className="pk-crop-side">
        <div className="pk-profile">
          {avatar ? (
            <img className="pk-avatar" src={avatar} alt="Your new avatar" />
          ) : (
            <span className="pk-avatar" data-empty="">
              MO
            </span>
          )}
          <div className="pk-profile-text">
            <p className="pk-profile-name">Maya Okafor</p>
            <p className="pk-profile-meta">
              {avatar ? "Photo updated" : "No photo yet"}
            </p>
          </div>
        </div>
        <label className="pk-zoom">
          <span className="b-label">Zoom</span>
          <ImageCropperZoom className="pk-zoom-range" />
        </label>
        <div className="pk-crop-actions">
          <ImageCropperRotate className="b-btn pk-icon-btn" aria-label="Rotate">
            <ArrowClockwiseIcon size={15} weight="bold" aria-hidden />
          </ImageCropperRotate>
          <Button
            className="b-btn pk-save"
            data-variant="primary"
            disabled={!crop || isSaving}
            onClick={save}
          >
            Save photo
          </Button>
        </div>
      </div>
    </ImageCropper>
  )
}

/* 4. Pickup time */

const pickupFormat = new Intl.DateTimeFormat("en-US", {
  weekday: "short",
  month: "short",
  day: "numeric",
})
const pickupTimeFormat = new Intl.DateTimeFormat("en-US", {
  hour: "numeric",
  minute: "2-digit",
})

const deskOpens = "06:00"
const deskCloses = "23:00"

function pickupSummary(date: string | null, time: string | null) {
  if (!date || !time) {
    return "Pick a day and time."
  }
  if (time < deskOpens || time > deskCloses) {
    return "The desk is open 6 AM to 11 PM."
  }
  const at = new Date(`${date}T${time}`)
  return `${pickupFormat.format(at)} at ${pickupTimeFormat.format(at)}`
}

function PickupTime() {
  const [date, setDate] = useState<string | null>("2026-11-20")
  const [time, setTime] = useState<string | null>("14:30")
  return (
    <div className="pk-pickup">
      <p className="pk-pickup-place">
        <VanIcon size={16} weight="fill" aria-hidden />
        Lisbon Airport, Terminal 1
      </p>
      <div className="pk-pickup-fields">
        <Field className="pk-pickup-field">
          <FieldLabel className="b-label">Date</FieldLabel>
          <DateInput
            className="b-input pk-segments"
            min="2026-11-18"
            value={date}
            onValueChange={setDate}
          />
        </Field>
        <Field className="pk-pickup-field">
          <FieldLabel className="b-label">Time</FieldLabel>
          <TimeInput
            className="b-input pk-segments"
            min={deskOpens}
            max={deskCloses}
            value={time}
            onValueChange={setTime}
          />
        </Field>
      </div>
      <p className="pk-pickup-summary" aria-live="polite">
        {pickupSummary(date, time)}
      </p>
    </div>
  )
}

/* 5. Stay rating */

const ratingWords = ["", "Poor", "Fair", "Good", "Very good", "Superb"]
const aspects = ["Cleanliness", "Location"]

function Star({ size }: { size: number }) {
  return (
    <RatingItem className="pk-star">
      <StarIcon size={size} weight="fill" aria-hidden />
      <StarIcon
        className="pk-star-fill"
        size={size}
        weight="fill"
        aria-hidden
      />
    </RatingItem>
  )
}

function StayRating() {
  const id = useId()
  const [overall, setOverall] = useState<number | null>(4.5)
  return (
    <div className="pk-stay">
      <div className="pk-stay-row" data-main="">
        <Label className="pk-stay-label" htmlFor={`${id}-overall`}>
          Overall
        </Label>
        <span className="pk-stay-word">
          {overall
            ? ratingWords[Math.max(1, Math.floor(overall))]
            : "Not rated"}
        </span>
      </div>
      <Rating
        id={`${id}-overall`}
        className="pk-rating"
        step={0.5}
        value={overall}
        onValueChange={setOverall}
      >
        <Star size={28} />
      </Rating>
      <div className="pk-stay-aspects">
        {aspects.map((aspect) => (
          <div key={aspect} className="pk-stay-row">
            <Label className="pk-stay-label" htmlFor={`${id}-${aspect}`}>
              {aspect}
            </Label>
            <Rating
              id={`${id}-${aspect}`}
              className="pk-rating"
              data-small=""
              defaultValue={aspect === "Location" ? 5 : 4}
            >
              <Star size={16} />
            </Rating>
          </div>
        ))}
      </div>
    </div>
  )
}

export const tiles: Tile[] = [
  {
    id: "forms-brand-color",
    title: "Pick the brand color.",
    blurb: "Drag, paste a hex or tap a preset; the site follows.",
    size: "large",
    Demo: BrandColor,
  },
  {
    id: "forms-contrast",
    title: "Will it pass AA?",
    blurb: "Any CSS color in, a WCAG ratio out.",
    size: "tall",
    tone: "paper",
    Demo: ContrastCheck,
  },
  {
    id: "forms-avatar-crop",
    title: "Frame your avatar.",
    blurb: "Drag to pan, scroll to zoom, then save.",
    size: "wide",
    tone: "ink",
    Demo: AvatarCrop,
  },
  {
    id: "forms-pickup-time",
    title: "Set the pickup time.",
    blurb: "Type or arrow through each part.",
    tone: "sky",
    Demo: PickupTime,
  },
  {
    id: "forms-stay-rating",
    title: "Rate your stay.",
    blurb: "Half stars by pointer or arrow keys.",
    tone: "butter",
    Demo: StayRating,
  },
]
