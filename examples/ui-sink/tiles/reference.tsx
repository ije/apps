import {
  BluetoothIcon,
  SpeakerHighIcon,
  SpeakerLowIcon,
  TextAlignCenterIcon,
  TextAlignLeftIcon,
  TextAlignRightIcon,
  TextBIcon,
  TextItalicIcon,
  TextUnderlineIcon,
  WifiHighIcon,
} from "@phosphor-icons/react"
import { Meter, MeterIndicator, MeterTrack } from "@ui-sdk/react/meter"
import {
  Slider,
  SliderControl,
  SliderIndicator,
  SliderThumb,
  SliderTrack,
} from "@ui-sdk/react/slider"
import { Switch, SwitchThumb } from "@ui-sdk/react/switch"
import { Toggle, ToggleGroup } from "@ui-sdk/react/toggle"
import { Toolbar, ToolbarSeparator } from "@ui-sdk/react/toolbar"
import { useState } from "react"
import type { Tile } from "../bento/types"
import "./reference.css"

function Radios() {
  const rows = [
    { label: "Wi-Fi", detail: "Studio 5G", icon: WifiHighIcon, on: true },
    {
      label: "Bluetooth",
      detail: "AirPods Pro",
      icon: BluetoothIcon,
      on: false,
    },
  ]
  return (
    <div className="ref-radios">
      {rows.map(({ label, detail, icon: Icon, on }) => (
        <label key={label} className="ref-radio">
          <span className="ref-radio-icon">
            <Icon weight="fill" size={16} />
          </span>
          <span className="ref-radio-text">
            <span>{label}</span>
            <span className="ref-radio-detail">{detail}</span>
          </span>
          <Switch className="ref-switch" defaultChecked={on}>
            <SwitchThumb className="ref-switch-thumb" />
          </Switch>
        </label>
      ))}
    </div>
  )
}

function Storage() {
  return (
    <div className="ref-storage">
      <p className="ref-storage-lead">Using</p>
      <p className="ref-storage-number">
        14.2<span> GB</span>
      </p>
      <Meter value={71} aria-label="Storage used" className="ref-meter">
        <MeterTrack className="ref-meter-track">
          <MeterIndicator className="ref-meter-fill" />
        </MeterTrack>
      </Meter>
      <p className="ref-storage-of">of 20 GB on your plan</p>
    </div>
  )
}

function Volume() {
  const [volume, setVolume] = useState(64)
  return (
    <div className="ref-volume">
      <Slider
        value={volume}
        onValueChange={(value) => setVolume(value as number)}
        className="ref-slider"
      >
        <SpeakerLowIcon weight="fill" size={16} aria-hidden />
        <SliderControl className="ref-slider-control">
          <SliderTrack className="ref-slider-track">
            <SliderIndicator className="ref-slider-fill" />
            <SliderThumb aria-label="Volume" className="ref-slider-thumb" />
          </SliderTrack>
        </SliderControl>
        <SpeakerHighIcon weight="fill" size={16} aria-hidden />
      </Slider>
      <p className="ref-volume-value">{volume}%</p>
    </div>
  )
}

function Formatting() {
  const [marks, setMarks] = useState(["bold"])
  const [align, setAlign] = useState(["left"])
  const style = {
    fontWeight: marks.includes("bold") ? 600 : 400,
    fontStyle: marks.includes("italic") ? "italic" : "normal",
    textDecoration: marks.includes("underline") ? "underline" : "none",
    textAlign: align[0] as "left" | "center" | "right",
  }
  return (
    <div className="ref-format">
      <Toolbar aria-label="Formatting" className="ref-toolbar">
        <ToggleGroup
          multiple
          aria-label="Text style"
          value={marks}
          onValueChange={setMarks}
          className="ref-group"
        >
          <Toggle value="bold" aria-label="Bold" className="ref-tool">
            <TextBIcon weight="bold" size={16} />
          </Toggle>
          <Toggle value="italic" aria-label="Italic" className="ref-tool">
            <TextItalicIcon weight="bold" size={16} />
          </Toggle>
          <Toggle value="underline" aria-label="Underline" className="ref-tool">
            <TextUnderlineIcon weight="bold" size={16} />
          </Toggle>
        </ToggleGroup>
        <ToolbarSeparator className="ref-sep" />
        <ToggleGroup
          aria-label="Alignment"
          value={align}
          deselectable={false}
          onValueChange={setAlign}
          className="ref-group"
        >
          <Toggle value="left" aria-label="Align left" className="ref-tool">
            <TextAlignLeftIcon weight="bold" size={16} />
          </Toggle>
          <Toggle value="center" aria-label="Align center" className="ref-tool">
            <TextAlignCenterIcon weight="bold" size={16} />
          </Toggle>
          <Toggle value="right" aria-label="Align right" className="ref-tool">
            <TextAlignRightIcon weight="bold" size={16} />
          </Toggle>
        </ToggleGroup>
      </Toolbar>
      <p className="ref-sample" style={style}>
        The quick brown fox
      </p>
    </div>
  )
}

export const tiles: Tile[] = [
  {
    id: "reference-switches",
    title: "Stay connected.",
    blurb: "Switches that slide and submit.",
    Demo: Radios,
  },
  {
    id: "reference-storage",
    title: "Know your quota.",
    blurb: "A meter, not a progress bar.",
    tone: "butter",
    Demo: Storage,
  },
  {
    id: "reference-volume",
    title: "Turn it up.",
    blurb: "Drag, click or use the arrows.",
    tone: "ink",
    Demo: Volume,
  },
  {
    id: "reference-format",
    title: "One tab stop.",
    blurb: "Two toggle groups inside a toolbar.",
    tone: "paper",
    Demo: Formatting,
  },
]
