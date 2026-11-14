// A flat API over libghostty-vt for src/lib.rs. All functions except vt_frame_end and vt_png need the lock
// of the terminal, because the PTY thread writes to it.
#include <ghostty/vt.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
  void *user;
  void (*reply)(void *user, const uint8_t *data, size_t len);
  // kind: 0 bell, 1 title, 2 working directory, 3 full reset, 4 new shell prompt, 5 and 6: the program
  // holds the screen for a synchronized update, and lets it go
  void (*event)(void *user, uint32_t kind, const uint8_t *text, size_t len);
  // strings: id, app, title, message
  void (*status)(void *user, uint32_t state, uint32_t kind, int32_t progress, const GhosttyString *strings);
} VtHost;

// Colors are 0x01RRGGBB, or 0 for the default color.
typedef struct {
  uint32_t ch, fg, bg, underline;
  // Bits 0 to 10 are GhosttyStyleFlags. 11: wide, 12: spacer, 13: selected.
  uint16_t flags;
  // The count of codepoints after `ch`. They are in `extras`, in cell order.
  uint16_t extra;
} VtCell;

// A rectangle of an image on the screen (Kitty graphics).
typedef struct {
  // The generation of the image. It is the key of the pixels, and no other image ever has it.
  uint64_t image;
  uint32_t id;
  // 0: below the backgrounds of the cells, 1: below the text, 2: above the text
  uint32_t layer;
  int32_t z;
  // The top left cell, which can be outside the grid.
  int32_t col, row;
  // In pixels: the offset in that cell, the size, and the part of the image to show.
  uint32_t x, y, width, height;
  uint32_t source[4];
} VtPlacement;

typedef struct {
  uint16_t cols, rows;
  uint16_t cursor_x, cursor_y;
  // 0: hidden, else 1 + GhosttyCursorStyle
  uint8_t cursor;
  uint8_t cursor_blinks;
  uint8_t cursor_wide;
  uint8_t full;
  uint32_t foreground, background, cursor_color;
  uint32_t count;
  const uint16_t *ys;
  const VtCell *cells;
  const uint32_t *extras;
  // The images in the order to draw them. All of them are in each frame.
  const VtPlacement *placements;
  uint32_t placed;
  // The time in ms until an animated image has its next picture, or 0.
  uint32_t wait;
  // In rows: all rows with the scrollback, the first row of the screen, and the rows of the screen.
  uint32_t scroll[3];
} VtFrame;

typedef uint8_t *(*VtAlloc)(void *out, size_t len);

typedef struct {
  uint16_t cols, rows, cursor_x, cursor_y;
  uint8_t cursor_visible, alternate, mouse, bottom;
  uint64_t total, offset;
  GhosttyString title, pwd;
} VtInfo;

typedef struct {
  GhosttyTerminal terminal;
  GhosttyRenderState render;
  GhosttyRenderStateRowIterator row_iter;
  GhosttyRenderStateRowCells row_cells;
  GhosttyRenderStateImageIterator image_iter;
  GhosttyKeyEncoder key_encoder;
  GhosttyKeyEvent key_event;
  GhosttyMouseEncoder mouse_encoder;
  GhosttyMouseEvent mouse_event;
  GhosttySelectionGesture gesture;
  GhosttySelectionGestureEvent gestures[4];
  VtHost host;
  uint16_t cols, rows;
  uint32_t cell_w, cell_h;
  bool dark, full;
  uint64_t ticked;
  uint16_t *ys;
  VtCell *cells;
  size_t cells_cap;
  uint32_t *extras;
  size_t extras_len, extras_cap;
  VtPlacement *placed;
  size_t placed_cap;
  uint32_t wait;
  uint32_t scroll[3];
} Vt;

static bool (*decode_png)(const uint8_t *data, size_t len, uint32_t *size, void *out, VtAlloc alloc);

typedef struct {
  const GhosttyAllocator *allocator;
  GhosttySysImage *image;
} Decoded;

static uint8_t *decoded(void *out, size_t len) {
  Decoded *target = out;
  target->image->data = ghostty_alloc(target->allocator, len);
  target->image->data_len = len;
  return target->image->data;
}

static bool on_png(void *user, const GhosttyAllocator *allocator, const uint8_t *data, size_t len,
                   GhosttySysImage *image) {
  Decoded target = {allocator, image};
  uint32_t size[2] = {0};
  if (decode_png(data, len, size, &target, decoded)) {
    image->width = size[0];
    image->height = size[1];
    return true;
  }
  if (image->data) ghostty_free(allocator, image->data, image->data_len);
  return false;
}

// Sets the PNG decoder for the images of all terminals. It writes the size and puts RGBA pixels in the
// memory from `alloc`, which can be NULL when the image is too large.
void vt_png(bool (*decode)(const uint8_t *data, size_t len, uint32_t *size, void *out, VtAlloc alloc)) {
  decode_png = decode;
  ghostty_sys_set(GHOSTTY_SYS_OPT_DECODE_PNG, (const void *)on_png);
}

static void on_write(GhosttyTerminal terminal, void *user, const uint8_t *data, size_t len) {
  Vt *vt = user;
  vt->host.reply(vt->host.user, data, len);
}

static void on_bell(GhosttyTerminal terminal, void *user) {
  Vt *vt = user;
  vt->host.event(vt->host.user, 0, NULL, 0);
}

static void on_title(GhosttyTerminal terminal, void *user) {
  Vt *vt = user;
  GhosttyString text = {0};
  ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_TITLE, &text);
  vt->host.event(vt->host.user, 1, text.ptr, text.len);
}

static void on_pwd(GhosttyTerminal terminal, void *user) {
  Vt *vt = user;
  GhosttyString text = {0};
  ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_PWD, &text);
  vt->host.event(vt->host.user, 2, text.ptr, text.len);
}

static void on_reset(GhosttyTerminal terminal, void *user) {
  Vt *vt = user;
  vt->host.event(vt->host.user, 3, NULL, 0);
}

static void on_prompt(GhosttyTerminal terminal, void *user, const GhosttyTerminalSemanticPrompt *event) {
  Vt *vt = user;
  if (event->kind == GHOSTTY_SEMANTIC_PROMPT_PROMPT_START &&
      event->prompt_kind == GHOSTTY_SEMANTIC_PROMPT_PROMPT_PRIMARY)
    vt->host.event(vt->host.user, 4, NULL, 0);
}

static void on_hold(GhosttyTerminal terminal, void *user, bool held) {
  Vt *vt = user;
  vt->host.event(vt->host.user, held ? 5 : 6, NULL, 0);
}

static void on_status(GhosttyTerminal terminal, void *user, const GhosttyTerminalProgramStatus *report) {
  Vt *vt = user;
  GhosttyString strings[4] = {report->id, report->app, report->title, report->message};
  vt->host.status(vt->host.user, report->state, report->kind, report->progress, strings);
}

static bool on_size(GhosttyTerminal terminal, void *user, GhosttySizeReportSize *size) {
  Vt *vt = user;
  size->rows = vt->rows;
  size->columns = vt->cols;
  size->cell_width = vt->cell_w;
  size->cell_height = vt->cell_h;
  return true;
}

static bool on_scheme(GhosttyTerminal terminal, void *user, GhosttyColorScheme *scheme) {
  Vt *vt = user;
  *scheme = vt->dark ? GHOSTTY_COLOR_SCHEME_DARK : GHOSTTY_COLOR_SCHEME_LIGHT;
  return true;
}

void vt_free(Vt *vt) {
  if (!vt) return;
  for (int i = 0; i < 4; i++) ghostty_selection_gesture_event_free(vt->gestures[i]);
  ghostty_selection_gesture_free(vt->gesture, vt->terminal);
  ghostty_mouse_event_free(vt->mouse_event);
  ghostty_mouse_encoder_free(vt->mouse_encoder);
  ghostty_key_event_free(vt->key_event);
  ghostty_key_encoder_free(vt->key_encoder);
  ghostty_render_state_row_cells_free(vt->row_cells);
  ghostty_render_state_image_iterator_free(vt->image_iter);
  ghostty_render_state_row_iterator_free(vt->row_iter);
  ghostty_render_state_free(vt->render);
  ghostty_terminal_free(vt->terminal);
  free(vt->ys);
  free(vt->cells);
  free(vt->extras);
  free(vt->placed);
  free(vt);
}

Vt *vt_new(uint16_t cols, uint16_t rows, const VtHost *host) {
  Vt *vt = calloc(1, sizeof(Vt));
  if (!vt) return NULL;
  vt->host = *host;
  vt->cols = cols;
  vt->rows = rows;
  vt->cell_w = 8;
  vt->cell_h = 16;
  vt->full = true;
  GhosttyTerminal terminal = NULL;
  bool ok = ghostty_terminal_new(NULL, &terminal, cols, rows) == GHOSTTY_SUCCESS;
  vt->terminal = terminal;
  ok = ok && ghostty_render_state_new(NULL, &vt->render) == GHOSTTY_SUCCESS;
  ok = ok && ghostty_render_state_row_iterator_new(NULL, &vt->row_iter) == GHOSTTY_SUCCESS;
  ok = ok && ghostty_render_state_row_cells_new(NULL, &vt->row_cells) == GHOSTTY_SUCCESS;
  ok = ok && ghostty_render_state_image_iterator_new(NULL, &vt->image_iter) == GHOSTTY_SUCCESS;
  ok = ok && ghostty_key_encoder_new(NULL, &vt->key_encoder) == GHOSTTY_SUCCESS;
  ok = ok && ghostty_key_event_new(NULL, &vt->key_event) == GHOSTTY_SUCCESS;
  ok = ok && ghostty_mouse_encoder_new(NULL, &vt->mouse_encoder) == GHOSTTY_SUCCESS;
  ok = ok && ghostty_mouse_event_new(NULL, &vt->mouse_event) == GHOSTTY_SUCCESS;
  ok = ok && ghostty_selection_gesture_new(NULL, &vt->gesture) == GHOSTTY_SUCCESS;
  static const GhosttySelectionGestureEventType types[4] = {
      GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS,
      GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE,
      GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG,
      GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_AUTOSCROLL_TICK,
  };
  for (int i = 0; i < 4; i++)
    ok = ok && ghostty_selection_gesture_event_new(NULL, &vt->gestures[i], types[i]) == GHOSTTY_SUCCESS;
  if (!ok) {
    vt_free(vt);
    return NULL;
  }
  // Images need a cell size before the first frame gives the real one.
  ghostty_terminal_resize(terminal, cols, rows, vt->cell_w, vt->cell_h);
  ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_USERDATA, vt);
  ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_WRITE_PTY, (const void *)on_write);
  ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_BELL, (const void *)on_bell);
  ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_TITLE_CHANGED, (const void *)on_title);
  ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_PWD_CHANGED, (const void *)on_pwd);
  ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_RESET, (const void *)on_reset);
  ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_SEMANTIC_PROMPT, (const void *)on_prompt);
  ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_PROGRAM_STATUS, (const void *)on_status);
  ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_RENDER_HOLD, (const void *)on_hold);
  ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_SIZE, (const void *)on_size);
  ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_COLOR_SCHEME, (const void *)on_scheme);
  // The default of 10 MB is less than one image of the size of a large window.
  uint64_t limit = 64 << 20;
  ghostty_terminal_set(terminal, GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT, &limit);
  return vt;
}

void vt_write(Vt *vt, const uint8_t *data, size_t len) {
  ghostty_terminal_vt_write(vt->terminal, data, len);
}

// Clears the screen and the scrollback, and sets all modes to their defaults.
void vt_reset(Vt *vt) {
  ghostty_terminal_reset(vt->terminal);
  vt->full = true;
}

void vt_resize(Vt *vt, uint16_t cols, uint16_t rows, uint32_t cell_w, uint32_t cell_h) {
  vt->cols = cols;
  vt->rows = rows;
  vt->cell_w = cell_w;
  vt->cell_h = cell_h;
  ghostty_terminal_resize(vt->terminal, cols, rows, cell_w, cell_h);
}

static bool mode(Vt *vt, GhosttyMode mode) {
  GhosttyTerminalModeConfig config = {.mode = mode};
  return ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_MODE, &config) == GHOSTTY_SUCCESS &&
         config.value;
}

static GhosttyColorRgb rgb(uint32_t color) {
  return (GhosttyColorRgb){color >> 16, color >> 8, color};
}

// colors: foreground, background, cursor, then the first 16 palette colors.
void vt_theme(Vt *vt, const uint32_t *colors, bool dark) {
  GhosttyColorRgb color = rgb(colors[0]);
  ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, &color);
  color = rgb(colors[1]);
  ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, &color);
  color = rgb(colors[2]);
  ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, colors[2] ? &color : NULL);
  GhosttyColorRgb palette[256];
  ghostty_color_palette_default(palette);
  for (int i = 0; i < 16; i++)
    if (colors[3 + i]) palette[i] = rgb(colors[3 + i]);
  ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, palette);
  vt->full = true;
  if (vt->dark != dark && mode(vt, GHOSTTY_MODE_REPORT_COLOR_SCHEME)) {
    uint8_t report[16];
    size_t len = 0;
    GhosttyColorScheme scheme = dark ? GHOSTTY_COLOR_SCHEME_DARK : GHOSTTY_COLOR_SCHEME_LIGHT;
    if (ghostty_color_scheme_report_encode(scheme, report, sizeof(report), &len) == GHOSTTY_SUCCESS)
      vt->host.reply(vt->host.user, report, len);
  }
  vt->dark = dark;
}

static const struct {
  const char *name;
  GhosttyKey key;
} KEYS[] = {
    {"enter", GHOSTTY_KEY_ENTER},        {"tab", GHOSTTY_KEY_TAB},
    {"backspace", GHOSTTY_KEY_BACKSPACE}, {"escape", GHOSTTY_KEY_ESCAPE},
    {"space", GHOSTTY_KEY_SPACE},        {"up", GHOSTTY_KEY_ARROW_UP},
    {"down", GHOSTTY_KEY_ARROW_DOWN},    {"left", GHOSTTY_KEY_ARROW_LEFT},
    {"right", GHOSTTY_KEY_ARROW_RIGHT},  {"home", GHOSTTY_KEY_HOME},
    {"end", GHOSTTY_KEY_END},            {"pageup", GHOSTTY_KEY_PAGE_UP},
    {"pagedown", GHOSTTY_KEY_PAGE_DOWN}, {"delete", GHOSTTY_KEY_DELETE},
    {"insert", GHOSTTY_KEY_INSERT},
};

// Each key of the US layout: the plain character, the shifted character, and the key.
static const struct {
  char plain, shifted;
  GhosttyKey key;
} PUNCTUATION[] = {
    {'`', '~', GHOSTTY_KEY_BACKQUOTE},   {'\\', '|', GHOSTTY_KEY_BACKSLASH},
    {'[', '{', GHOSTTY_KEY_BRACKET_LEFT}, {']', '}', GHOSTTY_KEY_BRACKET_RIGHT},
    {',', '<', GHOSTTY_KEY_COMMA},       {'=', '+', GHOSTTY_KEY_EQUAL},
    {'-', '_', GHOSTTY_KEY_MINUS},       {'.', '>', GHOSTTY_KEY_PERIOD},
    {'\'', '"', GHOSTTY_KEY_QUOTE},      {';', ':', GHOSTTY_KEY_SEMICOLON},
    {'/', '?', GHOSTTY_KEY_SLASH},       {'0', ')', GHOSTTY_KEY_DIGIT_0},
    {'1', '!', GHOSTTY_KEY_DIGIT_1},     {'2', '@', GHOSTTY_KEY_DIGIT_2},
    {'3', '#', GHOSTTY_KEY_DIGIT_3},     {'4', '$', GHOSTTY_KEY_DIGIT_4},
    {'5', '%', GHOSTTY_KEY_DIGIT_5},     {'6', '^', GHOSTTY_KEY_DIGIT_6},
    {'7', '&', GHOSTTY_KEY_DIGIT_7},     {'8', '*', GHOSTTY_KEY_DIGIT_8},
    {'9', '(', GHOSTTY_KEY_DIGIT_9},
};

static GhosttyMods mods_of(uint32_t mods) {
  return (mods & 1 ? GHOSTTY_MODS_SHIFT : 0) | (mods & 2 ? GHOSTTY_MODS_CTRL : 0) |
         (mods & 4 ? GHOSTTY_MODS_ALT : 0) | (mods & 8 ? GHOSTTY_MODS_SUPER : 0);
}

static size_t encode_key(Vt *vt, GhosttyKeyAction action, GhosttyKey key, GhosttyMods mods,
                         GhosttyMods consumed, GhosttyString text, uint32_t unshifted, bool option,
                         uint8_t *out, size_t cap) {
  GhosttyOptionAsAlt alt = option ? GHOSTTY_OPTION_AS_ALT_TRUE : GHOSTTY_OPTION_AS_ALT_FALSE;
  ghostty_key_encoder_set_from_terminal(vt->key_encoder, vt->terminal);
  ghostty_key_encoder_set(vt->key_encoder, GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT, &alt);
  ghostty_key_event_set_action(vt->key_event, action);
  ghostty_key_event_set_key(vt->key_event, key);
  ghostty_key_event_set_mods(vt->key_event, mods);
  ghostty_key_event_set_consumed_mods(vt->key_event, consumed);
  ghostty_key_event_set_composing(vt->key_event, false);
  ghostty_key_event_set_utf8(vt->key_event, text);
  ghostty_key_event_set_unshifted_codepoint(vt->key_event, unshifted);
  size_t len = 0;
  GhosttyResult result = ghostty_key_encoder_encode(vt->key_encoder, vt->key_event, out, cap, &len);
  ghostty_key_event_set_utf8(vt->key_event, (GhosttyString){0});
  return result == GHOSTTY_SUCCESS ? len : 0;
}

// Encodes a key for the program. `name` is the key name of the renderer: a name like `enter` or `f5`, or
// the character of the key. `mods` has the bits shift 1, ctrl 2, alt 4 and super 8, and bit 16 makes the
// Option key of macOS an Alt key. Returns the byte count.
size_t vt_key(Vt *vt, uint32_t action, const char *name, size_t name_len, uint32_t mods,
              const uint8_t *text, size_t text_len, uint8_t *out, size_t cap) {
  GhosttyKey key = GHOSTTY_KEY_UNIDENTIFIED;
  uint32_t unshifted = 0;
  if (name_len == 1) {
    char c = name[0];
    if (c >= 'A' && c <= 'Z') c += 'a' - 'A';
    if (c >= 'a' && c <= 'z') {
      key = GHOSTTY_KEY_A + (c - 'a');
      unshifted = c;
    }
    for (size_t i = 0; !unshifted && i < sizeof(PUNCTUATION) / sizeof(PUNCTUATION[0]); i++)
      if (c == PUNCTUATION[i].plain || c == PUNCTUATION[i].shifted) {
        key = PUNCTUATION[i].key;
        unshifted = PUNCTUATION[i].plain;
      }
  } else if (name_len > 1 && name_len < 4 && name[0] == 'f' && name[1] >= '1' && name[1] <= '9') {
    int number = name[1] - '0';
    if (name_len == 3) number = name[2] >= '0' && name[2] <= '9' ? number * 10 + name[2] - '0' : 0;
    if (number >= 1 && number <= 25) key = GHOSTTY_KEY_F1 + (number - 1);
  } else {
    for (size_t i = 0; i < sizeof(KEYS) / sizeof(KEYS[0]); i++)
      if (strlen(KEYS[i].name) == name_len && !memcmp(KEYS[i].name, name, name_len)) key = KEYS[i].key;
    if (key == GHOSTTY_KEY_SPACE) unshifted = ' ';
  }
  // Control characters come from the key, not from the text.
  if (text_len && (text[0] < 0x20 || text[0] == 0x7f)) text_len = 0;
  GhosttyMods held = mods_of(mods), consumed = 0;
  if (text_len) {
    consumed = held & GHOSTTY_MODS_SHIFT;
#ifdef __APPLE__
    // Option types a character on macOS.
    if (!(mods & 16)) consumed |= held & GHOSTTY_MODS_ALT;
#endif
  }
  GhosttyKeyAction kind = action == 0   ? GHOSTTY_KEY_ACTION_RELEASE
                          : action == 2 ? GHOSTTY_KEY_ACTION_REPEAT
                                        : GHOSTTY_KEY_ACTION_PRESS;
  return encode_key(vt, kind, key, held, consumed, (GhosttyString){text, text_len}, unshifted, mods & 16,
                    out, cap);
}

// Sends pasted text to the program. It changes `data`.
void vt_paste(Vt *vt, char *data, size_t len) {
  bool bracketed = mode(vt, GHOSTTY_MODE_BRACKETED_PASTE);
  size_t needed = 0;
  if (ghostty_paste_encode(data, len, bracketed, NULL, 0, &needed) != GHOSTTY_OUT_OF_SPACE) return;
  uint8_t *encoded = malloc(needed);
  if (!encoded) return;
  if (ghostty_paste_encode(data, len, bracketed, encoded, needed, &needed) == GHOSTTY_SUCCESS)
    vt->host.reply(vt->host.user, encoded, needed);
  free(encoded);
}

void vt_focus(Vt *vt, bool focused) {
  if (!mode(vt, GHOSTTY_MODE_FOCUS_EVENT)) return;
  uint8_t report[8];
  size_t len = 0;
  GhosttyFocusEvent event = focused ? GHOSTTY_FOCUS_GAINED : GHOSTTY_FOCUS_LOST;
  if (ghostty_focus_encode(event, report, sizeof(report), &len) == GHOSTTY_SUCCESS)
    vt->host.reply(vt->host.user, report, len);
}

// kind: 0 top, 1 bottom, 2 by rows, 3 to a row from the top of the scrollback
void vt_scroll(Vt *vt, uint32_t kind, int32_t rows) {
  GhosttyTerminalScrollViewport behavior = {.tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_TOP};
  if (kind == 1) behavior.tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM;
  if (kind == 3) {
    behavior.tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_ROW;
    behavior.value.row = rows < 0 ? 0 : rows;
  }
  if (kind == 2) {
    behavior.tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA;
    behavior.value.delta = rows;
  }
  ghostty_terminal_scroll_viewport(vt->terminal, behavior);
}

static bool tracking(Vt *vt, uint32_t mods) {
  bool on = false;
  ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING, &on);
  // Shift keeps the mouse for selection and scrollback.
  return on && !(mods & 1);
}

static void report_mouse(Vt *vt, GhosttyMouseAction action, uint32_t button, uint32_t mods, double x,
                         double y, bool held) {
  GhosttyMouseEncoderSize size = GHOSTTY_INIT_SIZED(GhosttyMouseEncoderSize);
  size.screen_width = vt->cols * vt->cell_w;
  size.screen_height = vt->rows * vt->cell_h;
  size.cell_width = vt->cell_w;
  size.cell_height = vt->cell_h;
  bool track = true;
  ghostty_mouse_encoder_set_from_terminal(vt->mouse_encoder, vt->terminal);
  ghostty_mouse_encoder_set(vt->mouse_encoder, GHOSTTY_MOUSE_ENCODER_OPT_SIZE, &size);
  ghostty_mouse_encoder_set(vt->mouse_encoder, GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED, &held);
  ghostty_mouse_encoder_set(vt->mouse_encoder, GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL, &track);
  ghostty_mouse_event_set_action(vt->mouse_event, action);
  if (button)
    ghostty_mouse_event_set_button(vt->mouse_event, button);
  else
    ghostty_mouse_event_clear_button(vt->mouse_event);
  ghostty_mouse_event_set_mods(vt->mouse_event, mods_of(mods));
  ghostty_mouse_event_set_position(vt->mouse_event, (GhosttyMousePosition){x, y});
  uint8_t report[128];
  size_t len = 0;
  if (ghostty_mouse_encoder_encode(vt->mouse_encoder, vt->mouse_event, report, sizeof(report), &len) ==
          GHOSTTY_SUCCESS &&
      len)
    vt->host.reply(vt->host.user, report, len);
}

// A pointer event at a position in cell pixels, from the top left of the grid. kind: 0 press, 1 release,
// 2 move. button: 1 left, 2 right, 3 middle, or 0. Returns true if the program got the event.
bool vt_mouse(Vt *vt, uint32_t kind, uint32_t button, uint32_t mods, double x, double y, bool held,
              uint64_t time) {
  if (tracking(vt, mods)) {
    static const GhosttyMouseAction actions[3] = {GHOSTTY_MOUSE_ACTION_PRESS, GHOSTTY_MOUSE_ACTION_RELEASE,
                                                  GHOSTTY_MOUSE_ACTION_MOTION};
    report_mouse(vt, actions[kind], button, mods, x, y, held);
    return true;
  }
  if (button != 1 && !(kind == 2 && held)) return false;
  double max_x = (double)vt->cols * vt->cell_w - 1, max_y = (double)vt->rows * vt->cell_h - 1;
  double in_x = x < 0 ? 0 : x > max_x ? max_x : x, in_y = y < 0 ? 0 : y > max_y ? max_y : y;
  GhosttyPoint point = {.tag = GHOSTTY_POINT_TAG_VIEWPORT};
  point.value.coordinate.x = in_x / vt->cell_w;
  point.value.coordinate.y = in_y / vt->cell_h;
  GhosttyGridRef ref = {0};
  if (ghostty_terminal_grid_ref(vt->terminal, point, &ref) != GHOSTTY_SUCCESS) return false;
  GhosttySurfacePosition position = {x, y};
  GhosttySelectionGestureGeometry geometry = {vt->cols, vt->cell_w, 0, vt->rows * vt->cell_h};
  GhosttySelectionGestureEvent event = vt->gestures[kind];
  ghostty_selection_gesture_event_set(event, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF, &ref);
  GhosttySelection selection = {0};
  GhosttyResult result = GHOSTTY_NO_VALUE;
  if (kind == 0) {
    ghostty_selection_gesture_event_set(event, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION, &position);
    ghostty_selection_gesture_event_set(event, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_TIME_NS, &time);
    result = ghostty_selection_gesture_event(vt->gesture, vt->terminal, event, &selection);
    // A single press only sets the anchor, so it removes the old selection.
    if (result == GHOSTTY_NO_VALUE)
      ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_SELECTION, NULL);
  } else if (kind == 1) {
    ghostty_selection_gesture_event(vt->gesture, vt->terminal, event, NULL);
  } else {
    bool rectangle = mods & 4;
    ghostty_selection_gesture_event_set(event, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION, &position);
    ghostty_selection_gesture_event_set(event, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY, &geometry);
    ghostty_selection_gesture_event_set(event, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE, &rectangle);
    result = ghostty_selection_gesture_event(vt->gesture, vt->terminal, event, &selection);
    GhosttySelectionGestureAutoscroll scroll = GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_NONE;
    ghostty_selection_gesture_get(vt->gesture, vt->terminal, GHOSTTY_SELECTION_GESTURE_DATA_AUTOSCROLL,
                                  &scroll);
    // The renderer sends the held pointer again and again, which drives the scroll of a drag past an edge.
    if (scroll != GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_NONE && time - vt->ticked > 30000000) {
      vt->ticked = time;
      event = vt->gestures[3];
      ghostty_selection_gesture_event_set(event, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION, &position);
      ghostty_selection_gesture_event_set(event, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY, &geometry);
      ghostty_selection_gesture_event_set(event, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE, &rectangle);
      ghostty_selection_gesture_event_set(event, GHOSTTY_SELECTION_GESTURE_EVENT_OPT_VIEWPORT,
                                          &point.value.coordinate);
      GhosttySelection scrolled = {0};
      if (ghostty_selection_gesture_event(vt->gesture, vt->terminal, event, &scrolled) == GHOSTTY_SUCCESS) {
        selection = scrolled;
        result = GHOSTTY_SUCCESS;
      }
    }
  }
  if (result == GHOSTTY_SUCCESS) ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_SELECTION, &selection);
  return false;
}

// Scrolls by rows: down when positive. The program gets the wheel when it tracks the mouse, and arrow keys
// on the alternate screen.
void vt_wheel(Vt *vt, int32_t rows, uint32_t mods, double x, double y) {
  uint32_t count = rows < 0 ? -rows : rows;
  if (count > 32) count = 32;
  if (tracking(vt, mods)) {
    for (uint32_t i = 0; i < count; i++)
      report_mouse(vt, GHOSTTY_MOUSE_ACTION_PRESS, rows < 0 ? GHOSTTY_MOUSE_BUTTON_FOUR : GHOSTTY_MOUSE_BUTTON_FIVE,
                   mods, x, y, false);
    return;
  }
  GhosttyTerminalScreen screen = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
  ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &screen);
  if (screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE && mode(vt, GHOSTTY_MODE_MOUSE_ALTERNATE_SCROLL)) {
    for (uint32_t i = 0; i < count; i++) {
      uint8_t bytes[16];
      size_t len = encode_key(vt, GHOSTTY_KEY_ACTION_PRESS, rows < 0 ? GHOSTTY_KEY_ARROW_UP : GHOSTTY_KEY_ARROW_DOWN,
                              0, 0, (GhosttyString){0}, 0, false, bytes, sizeof(bytes));
      if (len) vt->host.reply(vt->host.user, bytes, len);
    }
    return;
  }
  vt_scroll(vt, 2, rows);
}

void vt_select(Vt *vt, bool all) {
  GhosttySelection selection = {0};
  if (all && ghostty_terminal_select_all(vt->terminal, &selection) == GHOSTTY_SUCCESS)
    ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_SELECTION, &selection);
  else
    ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_SELECTION, NULL);
}

// The selected text, or all text of the screen with its scrollback. Free it with vt_text_free.
bool vt_text(Vt *vt, bool selected, uint8_t **text, size_t *len) {
  GhosttySelection selection = {0};
  if (selected &&
      ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_SELECTION, &selection) != GHOSTTY_SUCCESS)
    return false;
  GhosttyFormatter formatter = NULL;
  if (ghostty_formatter_terminal_new(NULL, &formatter, vt->terminal) != GHOSTTY_SUCCESS) return false;
  bool unwrap = true;
  ghostty_formatter_set(formatter, GHOSTTY_FORMATTER_OPT_UNWRAP, &unwrap);
  if (selected) ghostty_formatter_set(formatter, GHOSTTY_FORMATTER_OPT_SELECTION, &selection);
  bool ok = ghostty_formatter_format_alloc(formatter, NULL, text, len) == GHOSTTY_SUCCESS;
  ghostty_formatter_free(formatter);
  return ok;
}

void vt_text_free(uint8_t *text, size_t len) {
  ghostty_free(NULL, text, len);
}

// The strings are valid until the next change of the terminal.
void vt_info(Vt *vt, VtInfo *info) {
  GhosttyTerminal terminal = vt->terminal;
  bool flag = false;
  GhosttyTerminalScreen screen = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
  GhosttyTerminalScrollbar bar = {0};
  memset(info, 0, sizeof(*info));
  info->cols = vt->cols;
  info->rows = vt->rows;
  ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_X, &info->cursor_x);
  ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_Y, &info->cursor_y);
  ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE, &flag);
  info->cursor_visible = flag;
  ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &screen);
  info->alternate = screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE;
  flag = false;
  ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING, &flag);
  info->mouse = flag;
  flag = true;
  ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE, &flag);
  info->bottom = flag;
  ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_SCROLLBAR, &bar);
  info->total = bar.total;
  info->offset = bar.offset;
  ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_TITLE, &info->title);
  ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_PWD, &info->pwd);
}

static GhosttyKittyGraphics graphics(Vt *vt) {
  GhosttyKittyGraphics graphics = NULL;
  ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_KITTY_GRAPHICS, &graphics);
  return graphics;
}

// `now` is a time in ms that never goes back. It moves the animated images to their next picture.
void vt_frame_begin(Vt *vt, uint64_t now) {
  uint64_t next = 0;
  vt->wait = 0;
  if (ghostty_kitty_graphics_animation_tick(graphics(vt), now, &next) == GHOSTTY_SUCCESS)
    vt->wait = next > now ? next - now : 1;
  GhosttyTerminalScrollbar bar = {0};
  ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_SCROLLBAR, &bar);
  vt->scroll[0] = bar.total, vt->scroll[1] = bar.offset, vt->scroll[2] = bar.len;
  ghostty_render_state_begin_update(vt->render, vt->terminal);
}

// Copies the pixels of an image as BGRA into the memory from `alloc`, and writes its size. Returns false
// when the image is not there with this generation.
bool vt_image(Vt *vt, uint32_t id, uint64_t generation, uint32_t *size, void *out, VtAlloc alloc) {
  GhosttyKittyGraphicsImage image = ghostty_kitty_graphics_image(graphics(vt), id);
  uint64_t found = 0;
  GhosttyKittyImageFormat format = GHOSTTY_KITTY_IMAGE_FORMAT_PNG;
  GhosttyString pixels = {0};
  ghostty_kitty_graphics_image_get(image, GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_GENERATION, &found);
  if (!image || found != generation) return false;
  ghostty_kitty_graphics_image_get(image, GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_WIDTH, &size[0]);
  ghostty_kitty_graphics_image_get(image, GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_HEIGHT, &size[1]);
  ghostty_kitty_graphics_image_get(image, GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_FORMAT, &format);
  ghostty_kitty_graphics_image_get(image, GHOSTTY_KITTY_GRAPHICS_IMAGE_DATA_PIXELS, &pixels);
  // The channel counts of GhosttyKittyImageFormat. Stored images are never PNG.
  static const uint8_t channels[5] = {3, 4, 0, 2, 1};
  size_t count = (size_t)size[0] * size[1], step = (unsigned)format < 5 ? channels[format] : 0;
  if (!step || pixels.len != count * step) return false;
  uint8_t *to = alloc(out, count * 4);
  if (!to) return false;
  bool gray = step < 3;
  for (const uint8_t *from = (const uint8_t *)pixels.ptr, *end = from + count * step; from < end;
       from += step, to += 4) {
    to[0] = from[gray ? 0 : 2];
    to[1] = from[gray ? 0 : 1];
    to[2] = from[0];
    to[3] = step == 4 ? from[3] : step == 2 ? from[1] : 255;
  }
  return true;
}

static int by_layer(const void *a, const void *b) {
  const VtPlacement *left = a, *right = b;
  if (left->layer != right->layer) return left->layer < right->layer ? -1 : 1;
  return (left->z > right->z) - (left->z < right->z);
}

static uint32_t color(GhosttyColorRgb rgb) {
  return 0x01000000u | (uint32_t)rgb.r << 16 | (uint32_t)rgb.g << 8 | rgb.b;
}

// Reads the rows that changed since the last frame. Only the thread that draws calls this, after
// vt_frame_begin, and it needs no lock. The pointers of `frame` are valid until the next call.
bool vt_frame_end(Vt *vt, VtFrame *frame) {
  GhosttyRenderState render = vt->render;
  memset(frame, 0, sizeof(*frame));
  if (ghostty_render_state_end_update(render) != GHOSTTY_SUCCESS) return false;
  GhosttyRenderStateDirty dirty = GHOSTTY_RENDER_STATE_DIRTY_FALSE;
  ghostty_render_state_get(render, GHOSTTY_RENDER_STATE_DATA_DIRTY, &dirty);
  ghostty_render_state_get(render, GHOSTTY_RENDER_STATE_DATA_COLS, &frame->cols);
  ghostty_render_state_get(render, GHOSTTY_RENDER_STATE_DATA_ROWS, &frame->rows);
  if (vt->full) dirty = GHOSTTY_RENDER_STATE_DIRTY_FULL;
  vt->full = false;
  GhosttyRenderStateColors colors = GHOSTTY_INIT_SIZED(GhosttyRenderStateColors);
  ghostty_render_state_get(render, GHOSTTY_RENDER_STATE_DATA_COLORS, &colors);
  frame->foreground = color(colors.foreground);
  frame->background = color(colors.background);
  frame->cursor_color = colors.cursor_has_value ? color(colors.cursor) : 0;
  GhosttyRenderStateCursor cursor = GHOSTTY_INIT_SIZED(GhosttyRenderStateCursor);
  ghostty_render_state_get(render, GHOSTTY_RENDER_STATE_DATA_CURSOR, &cursor);
  if (cursor.visible && cursor.viewport_has_value) {
    frame->cursor = 1 + cursor.visual_style;
    frame->cursor_x = cursor.viewport_x;
    frame->cursor_y = cursor.viewport_y;
    frame->cursor_blinks = cursor.blinking;
    frame->cursor_wide = cursor.wide_tail;
  }
  if (ghostty_render_state_get(render, GHOSTTY_RENDER_STATE_DATA_IMAGE_ITERATOR, &vt->image_iter) ==
      GHOSTTY_SUCCESS)
    while (ghostty_render_state_image_iterator_next(vt->image_iter)) {
      if (frame->placed == vt->placed_cap) {
        size_t cap = vt->placed_cap ? vt->placed_cap * 2 : 8;
        VtPlacement *placed = realloc(vt->placed, cap * sizeof(VtPlacement));
        if (!placed) break;
        vt->placed = placed;
        vt->placed_cap = cap;
      }
      VtPlacement *placement = vt->placed + frame->placed++;
      GhosttyImageLayer layer = GHOSTTY_IMAGE_LAYER_ABOVE_TEXT;
      GhosttyRenderStateImageIterator iter = vt->image_iter;
      memset(placement, 0, sizeof(*placement));
      ghostty_render_state_image_get(iter, GHOSTTY_RENDER_STATE_IMAGE_DATA_IMAGE_GENERATION, &placement->image);
      ghostty_render_state_image_get(iter, GHOSTTY_RENDER_STATE_IMAGE_DATA_IMAGE_ID, &placement->id);
      ghostty_render_state_image_get(iter, GHOSTTY_RENDER_STATE_IMAGE_DATA_LAYER, &layer);
      placement->layer = layer;
      ghostty_render_state_image_get(iter, GHOSTTY_RENDER_STATE_IMAGE_DATA_Z, &placement->z);
      ghostty_render_state_image_get(iter, GHOSTTY_RENDER_STATE_IMAGE_DATA_VIEWPORT_COL, &placement->col);
      ghostty_render_state_image_get(iter, GHOSTTY_RENDER_STATE_IMAGE_DATA_VIEWPORT_ROW, &placement->row);
      ghostty_render_state_image_get(iter, GHOSTTY_RENDER_STATE_IMAGE_DATA_CELL_OFFSET_X, &placement->x);
      ghostty_render_state_image_get(iter, GHOSTTY_RENDER_STATE_IMAGE_DATA_CELL_OFFSET_Y, &placement->y);
      ghostty_render_state_image_get(iter, GHOSTTY_RENDER_STATE_IMAGE_DATA_PIXEL_WIDTH, &placement->width);
      ghostty_render_state_image_get(iter, GHOSTTY_RENDER_STATE_IMAGE_DATA_PIXEL_HEIGHT, &placement->height);
      for (int i = 0; i < 4; i++)
        ghostty_render_state_image_get(iter, GHOSTTY_RENDER_STATE_IMAGE_DATA_SOURCE_X + i,
                                       &placement->source[i]);
    }
  qsort(vt->placed, frame->placed, sizeof(VtPlacement), by_layer);
  // An animation that is not on the screen needs no frames.
  frame->wait = frame->placed ? vt->wait : 0;
  memcpy(frame->scroll, vt->scroll, sizeof(vt->scroll));
  frame->placements = vt->placed;
  frame->full = dirty == GHOSTTY_RENDER_STATE_DIRTY_FULL;
  if (dirty == GHOSTTY_RENDER_STATE_DIRTY_FALSE) return true;
  size_t cols = frame->cols, needed = cols * frame->rows;
  if (needed > vt->cells_cap) {
    VtCell *cells = realloc(vt->cells, needed * sizeof(VtCell));
    if (cells) vt->cells = cells;
    uint16_t *ys = realloc(vt->ys, (size_t)frame->rows * sizeof(uint16_t));
    if (ys) vt->ys = ys;
    if (!cells || !ys) return false;
    vt->cells_cap = needed;
  }
  vt->extras_len = 0;
  if (ghostty_render_state_get(render, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, &vt->row_iter) !=
      GHOSTTY_SUCCESS)
    return false;
  for (uint16_t y = 0; y < frame->rows && ghostty_render_state_row_iterator_next(vt->row_iter); y++) {
    bool changed = true;
    if (!frame->full)
      ghostty_render_state_row_get(vt->row_iter, GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY, &changed);
    if (!changed) continue;
    VtCell *row = vt->cells + frame->count * cols;
    vt->ys[frame->count++] = y;
    memset(row, 0, cols * sizeof(VtCell));
    if (ghostty_render_state_row_get(vt->row_iter, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, &vt->row_cells) !=
        GHOSTTY_SUCCESS)
      continue;
    GhosttyRenderStateRowSelection selection = GHOSTTY_INIT_SIZED(GhosttyRenderStateRowSelection);
    bool selected = ghostty_render_state_row_get(vt->row_iter, GHOSTTY_RENDER_STATE_ROW_DATA_SELECTION,
                                                 &selection) == GHOSTTY_SUCCESS;
    for (size_t x = 0; x < cols && ghostty_render_state_row_cells_next(vt->row_cells); x++) {
      VtCell *cell = row + x;
      GhosttyCell raw = 0;
      GhosttyCellWide wide = GHOSTTY_CELL_WIDE_NARROW;
      ghostty_render_state_row_cells_get(vt->row_cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, &raw);
      ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, &wide);
      if (wide == GHOSTTY_CELL_WIDE_WIDE) cell->flags |= 1 << 11;
      if (wide == GHOSTTY_CELL_WIDE_SPACER_TAIL || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD)
        cell->flags |= 1 << 12;
      if (selected && x >= selection.start_x && x <= selection.end_x) cell->flags |= 1 << 13;
      uint32_t points[16];
      GhosttyCodepointBuffer text = {points, 16, 0};
      ghostty_render_state_row_cells_get(vt->row_cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF,
                                         &text);
      size_t count = text.len < 16 ? text.len : 16;
      // The placeholder character of an image draws nothing: the image is in the placements.
      if (count && points[0] != 0x10EEEE) {
        cell->ch = points[0];
        if (count > 1 && vt->extras_len + count > vt->extras_cap) {
          size_t cap = vt->extras_cap ? vt->extras_cap * 2 : 256;
          uint32_t *extras = realloc(vt->extras, cap * sizeof(uint32_t));
          if (extras) {
            vt->extras = extras;
            vt->extras_cap = cap;
          } else {
            count = 1;
          }
        }
        cell->extra = count - 1;
        for (size_t i = 1; i < count; i++) vt->extras[vt->extras_len++] = points[i];
      }
      GhosttyColorRgb value;
      if (ghostty_render_state_row_cells_get(vt->row_cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR,
                                             &value) == GHOSTTY_SUCCESS)
        cell->bg = color(value);
      bool styled = false;
      ghostty_render_state_row_cells_get(vt->row_cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING,
                                         &styled);
      if (!styled) continue;
      GhosttyStyle style;
      if (ghostty_render_state_row_cells_get(vt->row_cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE,
                                             &style) != GHOSTTY_SUCCESS)
        continue;
      cell->flags |= style.flags & 0x7ff;
      if (ghostty_render_state_row_cells_get(vt->row_cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR,
                                             &value) == GHOSTTY_SUCCESS)
        cell->fg = color(value);
      if (style.underline_color.tag == GHOSTTY_STYLE_COLOR_RGB)
        cell->underline = color(style.underline_color.value.rgb);
      else if (style.underline_color.tag == GHOSTTY_STYLE_COLOR_PALETTE)
        cell->underline = color(colors.palette[style.underline_color.value.palette]);
    }
  }
  ghostty_render_state_clean(render);
  frame->ys = vt->ys;
  frame->cells = vt->cells;
  frame->extras = vt->extras;
  return true;
}
