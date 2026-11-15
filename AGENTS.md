# Agents Guide

A tiny, high performance, and low memory usage electron alternative with compatible API.

## Goals

- 120FPS vsync, 0 FPS when idle, low CPU load
- Small runtime size (less than 10MB)
- Fast launch (less than 300ms)
- Low memory usage (simple project less then 80mb)

## Important Rules

- When working on renderer, compare with chrome.
- Don't use workaroud/trick in testings/examples to make falsely the rendering look same as chrome.
- Build npm bin for current platform after cli/runtime/libs change.

## Code Style

- Add comments in code only it's vary important.
- Use Simple Tech English.
- Check code format and lint always.
- Use two spance indent.

## Architecture

see @docs/ARCHITECTURE.md

## Compatibility

see @docs/COMPATIBILITY.md
