# Windows Compatibility

These parts of the Windows backend differ from Chrome and Electron on Windows:

| Feature | Support |
|---|---|
| Mouse capture | The window holds the mouse while a button is down, but the release of any button ends the capture, also while other buttons stay down. A drag that goes on with another button then stops getting moves outside the window |
| Mouse wheel | With the "one screen at a time" wheel setting, one notch scrolls a box to its end, where Chrome scrolls one page |
| Frames | Each frame draws the whole window again, also when only a small part changed, so small changes cost more GPU time than on macOS |
