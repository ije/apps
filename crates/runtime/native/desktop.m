#import <AppKit/AppKit.h>
#import <Security/Security.h>
#import <CommonCrypto/CommonCrypto.h>
#import <UserNotifications/UserNotifications.h>
#import <Carbon/Carbon.h>
#import <ImageIO/ImageIO.h>
#import <IOKit/ps/IOPowerSources.h>
#import <IOKit/ps/IOPSKeys.h>
#import <ServiceManagement/ServiceManagement.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>
#include <stdint.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <sys/file.h>
#include <unistd.h>
#include <errno.h>

bool apps_reduce_motion(void) { return NSWorkspace.sharedWorkspace.accessibilityDisplayShouldReduceMotion; }

static NSString *AppsThemeSource = @"system";
static NSDictionary *AppsAboutOptions;
static void (*AppsWake)(void);
static _Atomic(uint32_t) AppsThemeCache = UINT32_MAX;

static uint32_t AppsReadTheme(void) {
    NSWorkspace *workspace = NSWorkspace.sharedWorkspace;
    NSAppearance *appearance = NSApp.effectiveAppearance ?: NSAppearance.currentDrawingAppearance;
    BOOL dark = [AppsThemeSource isEqual:@"dark"] || ([AppsThemeSource isEqual:@"system"] && [[appearance bestMatchFromAppearancesWithNames:@[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]] isEqual:NSAppearanceNameDarkAqua]);
    return dark | (workspace.accessibilityDisplayShouldIncreaseContrast << 1)
        | (workspace.accessibilityDisplayShouldInvertColors << 2)
        | (workspace.accessibilityDisplayShouldReduceTransparency << 3)
        | (workspace.accessibilityDisplayShouldDifferentiateWithoutColor << 4)
        | (workspace.accessibilityDisplayShouldReduceMotion << 5);
}

bool apps_display_p3(void) {
    static int p3 = -1;
    if (p3 < 0) p3 = [NSScreen.mainScreen canRepresentDisplayGamut:NSDisplayGamutP3];
    return p3;
}

bool apps_image_decode(const uint8_t *bytes, size_t length, uint8_t *(*allocate)(void *, size_t, size_t), void *context) {
    @autoreleasepool {
        CFDataRef data = CFDataCreateWithBytesNoCopy(NULL, bytes, length, kCFAllocatorNull);
        CGImageSourceRef source = CGImageSourceCreateWithData(data, NULL);
        CFRelease(data);
        if (!source) return false;
        // WARNING: only these web formats decode, so a file that works on macOS also works with the other platforms' decoders.
        NSString *type = (__bridge NSString *)CGImageSourceGetType(source);
        CGImageRef image = type && [@[@"org.webmproject.webp", @"com.microsoft.bmp", @"com.microsoft.ico", @"public.avif"] containsObject:type] ? CGImageSourceCreateImageAtIndex(source, 0, NULL) : NULL;
        CFRelease(source);
        if (!image) return false;
        size_t width = CGImageGetWidth(image), height = CGImageGetHeight(image);
        uint8_t *pixels = allocate(context, width, height);
        CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        CGContextRef bitmap = pixels ? CGBitmapContextCreate(pixels, width, height, 8, width * 4, space, kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little) : NULL;
        CGColorSpaceRelease(space);
        if (bitmap) CGContextDrawImage(bitmap, CGRectMake(0, 0, width, height), image);
        CGContextRelease(bitmap);
        CGImageRelease(image);
        return bitmap;
    }
}

uint32_t apps_theme_state(void) {
    uint32_t state = atomic_load_explicit(&AppsThemeCache, memory_order_relaxed);
    return state == UINT32_MAX ? AppsReadTheme() : state;
}

static NSString *AppsString(const char *text) { return text ? [NSString stringWithUTF8String:text] : @""; }
static NSString *AppsJSON(id value) {
    if (!value) value = [NSNull null];
    NSData *data = [NSJSONSerialization dataWithJSONObject:value options:NSJSONWritingFragmentsAllowed error:nil];
    return data ? [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] : @"null";
}
static id AppsValue(const char *text) {
    NSData *data = [AppsString(text) dataUsingEncoding:NSUTF8StringEncoding];
    return [NSJSONSerialization JSONObjectWithData:data options:NSJSONReadingFragmentsAllowed error:nil];
}

static NSData *AppsSecret(OSStatus *status) {
    NSString *service = [NSBundle.mainBundle.bundleIdentifier ?: @"org.apps.runtime" stringByAppendingString:@".safe-storage"];
    NSDictionary *query = @{ (__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword, (__bridge id)kSecAttrService: service, (__bridge id)kSecAttrAccount: @"encryption-key", (__bridge id)kSecReturnData: @YES };
    CFTypeRef existing = NULL;
    *status = SecItemCopyMatching((__bridge CFDictionaryRef)query, &existing);
    if (*status == errSecSuccess) return CFBridgingRelease(existing);
    if (*status != errSecItemNotFound) return nil;
    NSMutableData *secret = [NSMutableData dataWithLength:64];
    *status = SecRandomCopyBytes(kSecRandomDefault, secret.length, secret.mutableBytes);
    if (*status != errSecSuccess) return nil;
    NSMutableDictionary *item = [query mutableCopy];
    [item removeObjectForKey:(__bridge id)kSecReturnData];
    item[(__bridge id)kSecValueData] = secret;
    item[(__bridge id)kSecAttrAccessible] = (__bridge id)kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly;
    *status = SecItemAdd((__bridge CFDictionaryRef)item, NULL);
    if (*status == errSecDuplicateItem) {
        *status = SecItemCopyMatching((__bridge CFDictionaryRef)query, &existing);
        return *status == errSecSuccess ? CFBridgingRelease(existing) : nil;
    }
    return *status == errSecSuccess ? secret : nil;
}

static NSMutableArray *AppsNativeEvents(void) {
    static NSMutableArray *events;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ events = [NSMutableArray new]; });
    return events;
}
static void AppsNativeEmit(NSDictionary *event) {
    dispatch_async(dispatch_get_main_queue(), ^{
        NSMutableArray *events = AppsNativeEvents();
        if (events.count >= 4096) [events removeObjectAtIndex:0];
        [events addObject:event];
        if (AppsWake) AppsWake();
    });
}

@interface AppsNative : NSObject <UNUserNotificationCenterDelegate>
@property(nonatomic, strong) NSMutableDictionary *hotkeys;
@property(nonatomic) int lockFD;
@property(nonatomic, copy) NSString *instanceNotice;
@property(nonatomic) uint32_t themeState;
- (void)menuAction:(NSMenuItem *)item;
- (void)screensChanged:(NSNotification *)notification;
- (void)keyAction:(NSMenuItem *)item;
- (void)themeChanged:(NSNotification *)notification;
- (void)powerChanged:(NSNotification *)notification;
- (void)secondInstance:(NSNotification *)notification;
- (void)about:(id)sender;
@end
@implementation AppsNative
- (void)secondInstance:(NSNotification *)notification {
    if (self.lockFD < 0) return;
    NSString *message = notification.userInfo[@"message"];
    if (![message isKindOfClass:[NSString class]]) return;
    id value = AppsValue(message.UTF8String);
    if ([value isKindOfClass:[NSDictionary class]]) AppsNativeEmit(@{ @"type": @"app", @"event": @"second-instance", @"args": value[@"argv"] ?: @[], @"cwd": value[@"cwd"] ?: @"", @"data": value[@"data"] ?: [NSNull null] });
}
- (void)about:(id)sender {
    [NSApp orderFrontStandardAboutPanelWithOptions:AppsAboutOptions ?: @{}];
}
- (void)menuAction:(NSMenuItem *)item {
    if ([item.representedObject isEqual:@"checkbox"]) item.state = item.state == NSControlStateValueOn ? NSControlStateValueOff : NSControlStateValueOn;
    if ([item.representedObject isEqual:@"radio"]) {
        NSArray *items = item.menu.itemArray;
        NSInteger start = [items indexOfObject:item], end = start;
        while (start > 0 && [[items[start - 1] representedObject] isEqual:@"radio"]) start--;
        while (end + 1 < (NSInteger)items.count && [[items[end + 1] representedObject] isEqual:@"radio"]) end++;
        for (NSInteger i = start; i <= end; i++) [items[i] setState:items[i] == item ? NSControlStateValueOn : NSControlStateValueOff];
    }
    NSEventModifierFlags flags = NSEvent.modifierFlags;
    AppsNativeEmit(@{ @"type": @"menu", @"id": @(item.tag), @"checked": @(item.state == NSControlStateValueOn), @"shift": @((flags & NSEventModifierFlagShift) != 0), @"control": @((flags & NSEventModifierFlagControl) != 0), @"alt": @((flags & NSEventModifierFlagOption) != 0), @"meta": @((flags & NSEventModifierFlagCommand) != 0), @"accelerator": @(NSApp.currentEvent.type == NSEventTypeKeyDown) });
}
- (void)screensChanged:(NSNotification *)notification {
    AppsNativeEmit(@{ @"type": @"screen" });
}
- (void)keyAction:(NSMenuItem *)item {
    NSArray *command = item.representedObject;
    if ([command[0] length] && [NSApp sendAction:NSSelectorFromString(command[0]) to:nil from:item]) return;
    NSWindow *window = NSApp.keyWindow;
    NSResponder *responder = [command[2] boolValue] ? window.contentView : window.firstResponder;
    if (![responder isKindOfClass:NSClassFromString(@"AppsView")]) return;
    NSArray *key = command[1];
    NSString *text = [key[2] intValue] == kVK_Delete ? @"\177" : key[0];
    NSEvent *event = [NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:[key[1] unsignedIntegerValue] timestamp:NSProcessInfo.processInfo.systemUptime windowNumber:window.windowNumber context:nil characters:text charactersIgnoringModifiers:text isARepeat:NO keyCode:[key[2] unsignedShortValue]];
    [responder performKeyEquivalent:event];
}
- (void)themeChanged:(NSNotification *)notification {
    uint32_t state = AppsReadTheme();
    if (state == self.themeState) return;
    self.themeState = state;
    if (atomic_load_explicit(&AppsThemeCache, memory_order_relaxed) != UINT32_MAX) atomic_store_explicit(&AppsThemeCache, state, memory_order_relaxed);
    AppsNativeEmit(@{ @"type": @"theme" });
}
- (void)observeValueForKeyPath:(NSString *)keyPath ofObject:(id)object change:(NSDictionary *)change context:(void *)context {
    [self themeChanged:nil];
}
- (void)powerChanged:(NSNotification *)notification {
    NSDictionary *events = @{ NSWorkspaceWillSleepNotification: @"suspend", NSWorkspaceDidWakeNotification: @"resume", NSWorkspaceWillPowerOffNotification: @"shutdown", NSProcessInfoThermalStateDidChangeNotification: @"thermal-state-change" };
    NSString *event = events[notification.name];
    if (event) AppsNativeEmit(@{ @"type": @"power", @"event": event });
}
- (void)userNotificationCenter:(UNUserNotificationCenter *)center didReceiveNotificationResponse:(UNNotificationResponse *)response withCompletionHandler:(void (^)(void))completionHandler {
    AppsNativeEmit(@{ @"type": @"notification", @"id": response.notification.request.identifier, @"event": @"click" });
    completionHandler();
}
- (void)userNotificationCenter:(UNUserNotificationCenter *)center willPresentNotification:(UNNotification *)notification withCompletionHandler:(void (^)(UNNotificationPresentationOptions))completionHandler {
    completionHandler(UNNotificationPresentationOptionBanner | UNNotificationPresentationOptionSound);
}
@end

static OSStatus AppsHotkey(EventHandlerCallRef handler, EventRef event, void *data) {
    (void)handler; (void)data;
    EventHotKeyID key;
    OSStatus status = GetEventParameter(event, kEventParamDirectObject, typeEventHotKeyID, NULL, sizeof(key), NULL, &key);
    if (status == noErr) AppsNativeEmit(@{ @"type": @"shortcut", @"id": @(key.id) });
    return status;
}

static AppsNative *AppsNativeOwner(void) {
    static AppsNative *owner;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        owner = [AppsNative new];
        owner.lockFD = -1;
        owner.hotkeys = [NSMutableDictionary new];
        owner.themeState = apps_theme_state();
        [[NSDistributedNotificationCenter defaultCenter] addObserver:owner selector:@selector(themeChanged:) name:@"AppleInterfaceThemeChangedNotification" object:nil];
        [NSWorkspace.sharedWorkspace.notificationCenter addObserver:owner selector:@selector(themeChanged:) name:NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification object:nil];
        for (NSString *name in @[NSWorkspaceWillSleepNotification, NSWorkspaceDidWakeNotification, NSWorkspaceWillPowerOffNotification]) [[NSWorkspace sharedWorkspace].notificationCenter addObserver:owner selector:@selector(powerChanged:) name:name object:nil];
        [[NSNotificationCenter defaultCenter] addObserver:owner selector:@selector(powerChanged:) name:NSProcessInfoThermalStateDidChangeNotification object:nil];
        [[NSNotificationCenter defaultCenter] addObserver:owner selector:@selector(screensChanged:) name:NSApplicationDidChangeScreenParametersNotification object:nil];
        EventTypeSpec type = { kEventClassKeyboard, kEventHotKeyPressed };
        InstallApplicationEventHandler(AppsHotkey, 1, &type, NULL, NULL);
    });
    return owner;
}

void apps_theme_observe(void (*wake)(void)) {
    AppsWake = wake;
    NSApp.appearance = [AppsThemeSource isEqual:@"system"] ? nil : [NSAppearance appearanceNamed:[AppsThemeSource isEqual:@"dark"] ? NSAppearanceNameDarkAqua : NSAppearanceNameAqua];
    AppsNative *owner = AppsNativeOwner();
    atomic_store_explicit(&AppsThemeCache, owner.themeState, memory_order_relaxed);
    [NSApp addObserver:owner forKeyPath:@"effectiveAppearance" options:0 context:NULL];
    [owner themeChanged:nil];
}

static NSArray *AppsAccelerator(NSString *accelerator) {
    static NSDictionary *keys;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        NSMutableDictionary *map = [NSMutableDictionary new];
        const char *plain = "abcdefghijklmnopqrstuvwxyz0123456789-=,./[];'`\\", *shifted = ")!@#$%^&*(_+<>?{}:\"~|", *base = "0123456789-=,./[];'`\\";
        const int codes[] = { kVK_ANSI_A, kVK_ANSI_B, kVK_ANSI_C, kVK_ANSI_D, kVK_ANSI_E, kVK_ANSI_F, kVK_ANSI_G, kVK_ANSI_H, kVK_ANSI_I, kVK_ANSI_J, kVK_ANSI_K, kVK_ANSI_L, kVK_ANSI_M, kVK_ANSI_N, kVK_ANSI_O, kVK_ANSI_P, kVK_ANSI_Q, kVK_ANSI_R, kVK_ANSI_S, kVK_ANSI_T, kVK_ANSI_U, kVK_ANSI_V, kVK_ANSI_W, kVK_ANSI_X, kVK_ANSI_Y, kVK_ANSI_Z, kVK_ANSI_0, kVK_ANSI_1, kVK_ANSI_2, kVK_ANSI_3, kVK_ANSI_4, kVK_ANSI_5, kVK_ANSI_6, kVK_ANSI_7, kVK_ANSI_8, kVK_ANSI_9, kVK_ANSI_Minus, kVK_ANSI_Equal, kVK_ANSI_Comma, kVK_ANSI_Period, kVK_ANSI_Slash, kVK_ANSI_LeftBracket, kVK_ANSI_RightBracket, kVK_ANSI_Semicolon, kVK_ANSI_Quote, kVK_ANSI_Grave, kVK_ANSI_Backslash };
        const int keypad[] = { kVK_ANSI_Keypad0, kVK_ANSI_Keypad1, kVK_ANSI_Keypad2, kVK_ANSI_Keypad3, kVK_ANSI_Keypad4, kVK_ANSI_Keypad5, kVK_ANSI_Keypad6, kVK_ANSI_Keypad7, kVK_ANSI_Keypad8, kVK_ANSI_Keypad9 };
        const int functions[] = { kVK_F1, kVK_F2, kVK_F3, kVK_F4, kVK_F5, kVK_F6, kVK_F7, kVK_F8, kVK_F9, kVK_F10, kVK_F11, kVK_F12, kVK_F13, kVK_F14, kVK_F15, kVK_F16, kVK_F17, kVK_F18, kVK_F19, kVK_F20 };
        for (size_t i = 0; plain[i]; i++) map[[NSString stringWithFormat:@"%c", plain[i]]] = @[[NSString stringWithFormat:@"%c", plain[i]], @(codes[i]), @NO];
        for (size_t i = 0; shifted[i]; i++) map[[NSString stringWithFormat:@"%c", shifted[i]]] = @[[NSString stringWithFormat:@"%c", shifted[i]], map[[NSString stringWithFormat:@"%c", base[i]]][1], @YES];
        for (int i = 0; i < 10; i++) map[[NSString stringWithFormat:@"num%d", i]] = @[[NSString stringWithFormat:@"%d", i], @(keypad[i]), @NO];
        for (int i = 0; i < 24; i++) { unichar key = NSF1FunctionKey + i; map[[NSString stringWithFormat:@"f%d", i + 1]] = @[[NSString stringWithCharacters:&key length:1], @(i < 20 ? functions[i] : -1), @NO]; }
        NSString *(^function)(unichar) = ^NSString *(unichar key) { return [NSString stringWithCharacters:&key length:1]; };
        [map addEntriesFromDictionary:@{ @"plus": @[@"+", @(kVK_ANSI_Equal), @YES], @"space": @[@" ", @(kVK_Space), @NO], @"tab": @[@"\t", @(kVK_Tab), @NO], @"backspace": @[@"\b", @(kVK_Delete), @NO], @"delete": @[function(NSDeleteFunctionKey), @(kVK_ForwardDelete), @NO], @"insert": @[function(NSInsertFunctionKey), @(kVK_Help), @NO], @"return": @[@"\r", @(kVK_Return), @NO], @"enter": @[@"\r", @(kVK_Return), @NO], @"escape": @[@"\e", @(kVK_Escape), @NO], @"esc": @[@"\e", @(kVK_Escape), @NO],
            @"up": @[function(NSUpArrowFunctionKey), @(kVK_UpArrow), @NO], @"down": @[function(NSDownArrowFunctionKey), @(kVK_DownArrow), @NO], @"left": @[function(NSLeftArrowFunctionKey), @(kVK_LeftArrow), @NO], @"right": @[function(NSRightArrowFunctionKey), @(kVK_RightArrow), @NO], @"home": @[function(NSHomeFunctionKey), @(kVK_Home), @NO], @"end": @[function(NSEndFunctionKey), @(kVK_End), @NO], @"pageup": @[function(NSPageUpFunctionKey), @(kVK_PageUp), @NO], @"pagedown": @[function(NSPageDownFunctionKey), @(kVK_PageDown), @NO], @"printscreen": @[function(NSPrintScreenFunctionKey), @-1, @NO],
            @"numdec": @[@".", @(kVK_ANSI_KeypadDecimal), @NO], @"numadd": @[@"+", @(kVK_ANSI_KeypadPlus), @NO], @"numsub": @[@"-", @(kVK_ANSI_KeypadMinus), @NO], @"nummult": @[@"*", @(kVK_ANSI_KeypadMultiply), @NO], @"numdiv": @[@"/", @(kVK_ANSI_KeypadDivide), @NO],
            @"volumeup": @[@"", @(kVK_VolumeUp), @NO], @"volumedown": @[@"", @(kVK_VolumeDown), @NO], @"volumemute": @[@"", @(kVK_Mute), @NO], @"capslock": @[@"", @(kVK_CapsLock), @NO], @"numlock": @[@"", @-1, @NO], @"scrolllock": @[@"", @-1, @NO], @"mediaplaypause": @[@"", @-1, @NO], @"medianexttrack": @[@"", @-1, @NO], @"mediaprevioustrack": @[@"", @-1, @NO], @"mediastop": @[@"", @-1, @NO] }];
        keys = map;
    });
    NSArray *parts = [accelerator componentsSeparatedByString:@"+"];
    NSArray *key = keys[[parts.lastObject lowercaseString]];
    NSUInteger modifiers = 0;
    for (NSUInteger i = 0; i + 1 < parts.count; i++) {
        NSString *part = [parts[i] lowercaseString];
        if ([@[@"command", @"cmd", @"commandorcontrol", @"cmdorctrl", @"super", @"meta"] containsObject:part]) modifiers |= NSEventModifierFlagCommand;
        else if ([@[@"control", @"ctrl"] containsObject:part]) modifiers |= NSEventModifierFlagControl;
        else if ([@[@"alt", @"option", @"altgr"] containsObject:part]) modifiers |= NSEventModifierFlagOption;
        else if ([part isEqual:@"shift"]) modifiers |= NSEventModifierFlagShift;
        else return nil;
    }
    return key ? @[key[0], @(modifiers), key[1], key[2]] : nil;
}

static NSMenu *AppsMenu(NSArray *template) {
    NSMenu *menu = [[NSMenu alloc] initWithTitle:@""];
    menu.autoenablesItems = NO;
    NSDictionary *roles = @{ @"about": @"about:", @"hide": @"hide:", @"hideothers": @"hideOtherApplications:", @"unhide": @"unhideAllApplications:", @"delete": @"delete:", @"startspeaking": @"startSpeaking:", @"stopspeaking": @"stopSpeaking:", @"front": @"arrangeInFront:", @"quit": @"terminate:", @"copy": @"copy:", @"paste": @"paste:", @"pasteandmatchstyle": @"pasteAsPlainText:", @"cut": @"cut:", @"selectall": @"selectAll:", @"undo": @"undo:", @"redo": @"redo:", @"minimize": @"performMiniaturize:", @"zoom": @"performZoom:", @"close": @"performClose:", @"togglefullscreen": @"toggleFullScreen:", @"toggledevtools": @"",
        @"toggletabbar": @"toggleTabBar:", @"selectnexttab": @"selectNextTab:", @"selectprevioustab": @"selectPreviousTab:", @"showalltabs": @"toggleTabOverview:", @"mergeallwindows": @"mergeAllWindows:", @"movetabtonewwindow": @"moveTabToNewWindow:", @"showsubstitutions": @"orderFrontSubstitutionsPanel:", @"togglesmartquotes": @"toggleAutomaticQuoteSubstitution:", @"togglesmartdashes": @"toggleAutomaticDashSubstitution:", @"toggletextreplacement": @"toggleAutomaticTextReplacement:", @"togglespellchecker": @"toggleContinuousSpellChecking:", @"clearrecentdocuments": @"clearRecentDocuments:" };
    NSDictionary *keys = @{ @"copy": @"Command+C", @"cut": @"Command+X", @"paste": @"Command+V", @"pasteandmatchstyle": @"Command+V", @"selectall": @"Command+A", @"undo": @"Command+Z", @"redo": @"Command+Shift+Z", @"delete": @"Backspace", @"toggledevtools": @"Command+Alt+I" };
    AppsNative *owner = AppsNativeOwner();
    for (NSDictionary *description in template) {
        if ([description[@"type"] isEqual:@"separator"]) { [menu addItem:[NSMenuItem separatorItem]]; continue; }
        NSString *role = description[@"role"], *selector = description[@"submenu"] ? nil : roles[role];
        NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:description[@"label"] ?: @"" action:selector.length ? NSSelectorFromString(selector) : @selector(menuAction:) keyEquivalent:@""];
        if (!selector || [role isEqual:@"about"]) { item.target = owner; item.tag = [description[@"click"] integerValue]; }
        NSArray *accelerator = description[@"accelerator"] ? AppsAccelerator(description[@"accelerator"]) : nil;
        if ([accelerator[0] length]) {
            item.keyEquivalent = accelerator[0];
            item.keyEquivalentModifierMask = [accelerator[1] unsignedIntegerValue];
        }
        item.enabled = description[@"enabled"] ? [description[@"enabled"] boolValue] : YES;
        item.hidden = description[@"visible"] ? ![description[@"visible"] boolValue] : NO;
        item.state = [description[@"checked"] boolValue] ? NSControlStateValueOn : NSControlStateValueOff;
        item.representedObject = description[@"type"];
        if (description[@"toolTip"]) item.toolTip = description[@"toolTip"];
        if (selector && keys[role]) {
            item.target = owner;
            item.action = @selector(keyAction:);
            item.representedObject = @[selector, AppsAccelerator(keys[role]), @([role isEqual:@"toggledevtools"])];
        }
        if ([role isEqual:@"services"] && !description[@"submenu"]) { item.submenu = [[NSMenu alloc] initWithTitle:@"Services"]; NSApp.servicesMenu = item.submenu; }
        if (description[@"submenu"]) {
            item.submenu = AppsMenu(description[@"submenu"]);
            item.submenu.title = item.title;
            if ([role isEqual:@"windowmenu"] || [role isEqual:@"window"]) NSApp.windowsMenu = item.submenu;
            if ([role isEqual:@"help"]) NSApp.helpMenu = item.submenu;
        }
        [menu addItem:item];
    }
    return menu;
}

char *apps_native_call(const char *operation, const char *argumentsJSON) {
    NSString *op = AppsString(operation);
    id args = AppsValue(argumentsJSON);
    id result = [NSNull null];
    if (![NSThread isMainThread]) return strdup("{\"error\":\"Native desktop APIs require the main thread\"}");
    if (([op hasPrefix:@"dialog."] || [op hasPrefix:@"menu."] || [op hasPrefix:@"shortcut."]) && !NSApp) return strdup("{\"error\":\"This operation requires a native app on the main thread\"}");
    if ([op isEqualToString:@"poll"]) { result = [AppsNativeEvents() copy]; [AppsNativeEvents() removeAllObjects]; }
    else if ([op isEqualToString:@"app.singleInstance"]) {
        AppsNative *owner = AppsNativeOwner();
        if (owner.lockFD >= 0) result = @YES;
        else {
            NSData *identifier = [args[@"id"] dataUsingEncoding:NSUTF8StringEncoding];
            unsigned char digest[CC_SHA256_DIGEST_LENGTH];
            CCHmac(kCCHmacAlgSHA256, "apps-instance", sizeof("apps-instance"), identifier.bytes, identifier.length, digest);
            NSMutableString *name = [NSMutableString stringWithString:@"org.apps.instance."];
            for (size_t i = 0; i < sizeof(digest); i++) [name appendFormat:@"%02x", digest[i]];
            if (owner.instanceNotice) [[NSDistributedNotificationCenter defaultCenter] removeObserver:owner name:owner.instanceNotice object:nil];
            owner.instanceNotice = name;
            [[NSDistributedNotificationCenter defaultCenter] addObserver:owner selector:@selector(secondInstance:) name:name object:nil suspensionBehavior:NSNotificationSuspensionBehaviorDeliverImmediately];
            NSString *path = [NSTemporaryDirectory() stringByAppendingPathComponent:name];
            int fd = open(path.fileSystemRepresentation, O_CREAT | O_RDWR | O_CLOEXEC | O_NOFOLLOW, 0600);
            if (fd < 0) return strdup("{\"error\":\"Cannot open application instance lock\"}");
            if (flock(fd, LOCK_EX | LOCK_NB) == 0) { owner.lockFD = fd; result = @YES; }
            else {
                int failure = errno; close(fd);
                if (failure != EWOULDBLOCK) return strdup("{\"error\":\"Cannot lock application instance\"}");
                [[NSDistributedNotificationCenter defaultCenter] postNotificationName:name object:nil userInfo:@{ @"message": AppsJSON(args) } deliverImmediately:YES];
                result = @NO;
            }
        }
    } else if ([op isEqualToString:@"app.releaseInstance"]) {
        AppsNative *owner = AppsNativeOwner();
        if (owner.lockFD >= 0) { flock(owner.lockFD, LOCK_UN); close(owner.lockFD); owner.lockFD = -1; }
    } else if ([op isEqualToString:@"app.hasInstance"]) result = @(AppsNativeOwner().lockFD >= 0);
    else if ([op isEqualToString:@"power.init"]) AppsNativeOwner();
    else if ([op isEqualToString:@"power.idle"]) result = @(CGEventSourceSecondsSinceLastEventType(kCGEventSourceStateHIDSystemState, kCGAnyInputEventType));
    else if ([op isEqualToString:@"power.thermal"]) result = @[@"nominal", @"fair", @"serious", @"critical"][MIN(3, MAX(0, NSProcessInfo.processInfo.thermalState))];
    else if ([op isEqualToString:@"power.battery"]) {
        CFTypeRef info = IOPSCopyPowerSourcesInfo();
        result = @(info && CFEqual(IOPSGetProvidingPowerSourceType(info), CFSTR(kIOPSBatteryPowerValue)));
        if (info) CFRelease(info);
    }
    else if ([op isEqualToString:@"menu.set"]) NSApp.mainMenu = args == [NSNull null] ? nil : AppsMenu(args);
    else if ([op isEqualToString:@"menu.popup"]) {
        NSMenu *menu = AppsMenu(args[@"template"]);
        NSView *view = NSApp.keyWindow.contentView;
        NSPoint point = args[@"x"] && args[@"y"] ? NSMakePoint([args[@"x"] doubleValue], [args[@"y"] doubleValue]) : [view convertPoint:NSApp.keyWindow.mouseLocationOutsideOfEventStream fromView:nil];
        if (view && !view.isFlipped && args[@"y"]) point.y = view.bounds.size.height - point.y;
        [menu popUpMenuPositioningItem:nil atLocation:point inView:view];
    } else if ([op isEqualToString:@"shortcut.register"]) {
        NSArray *accelerator = AppsAccelerator(args[@"accelerator"]);
        if (!accelerator) return strdup("{\"error\":\"Unsupported shortcut accelerator\"}");
        if ([accelerator[2] intValue] < 0) return strdup("{\"value\":false}");
        NSUInteger flags = [accelerator[1] unsignedIntegerValue] | ([accelerator[3] boolValue] ? NSEventModifierFlagShift : 0);
        UInt32 modifiers = (flags & NSEventModifierFlagCommand ? cmdKey : 0) | (flags & NSEventModifierFlagControl ? controlKey : 0) | (flags & NSEventModifierFlagOption ? optionKey : 0) | (flags & NSEventModifierFlagShift ? shiftKey : 0);
        EventHotKeyRef hotkey = NULL;
        EventHotKeyID key = { 'apps', [args[@"id"] unsignedIntValue] };
        AppsNative *owner = AppsNativeOwner();
        OSStatus status = RegisterEventHotKey([accelerator[2] unsignedIntValue], modifiers, key, GetApplicationEventTarget(), 0, &hotkey);
        if (status == noErr) owner.hotkeys[args[@"id"]] = [NSValue valueWithPointer:hotkey];
        result = @(status == noErr);
    } else if ([op isEqualToString:@"shortcut.unregister"]) {
        NSValue *hotkey = AppsNativeOwner().hotkeys[args];
        if (hotkey) { UnregisterEventHotKey(hotkey.pointerValue); [AppsNativeOwner().hotkeys removeObjectForKey:args]; }
    } else if ([op isEqualToString:@"clipboard.readText"]) result = [[NSPasteboard generalPasteboard] stringForType:NSPasteboardTypeString] ?: @"";
    else if ([op isEqualToString:@"clipboard.readHTML"]) result = [[NSPasteboard generalPasteboard] stringForType:NSPasteboardTypeHTML] ?: @"";
    else if ([op isEqualToString:@"clipboard.writeText"] || [op isEqualToString:@"clipboard.writeHTML"]) {
        NSPasteboard *board = [NSPasteboard generalPasteboard];
        [board clearContents];
        [board setString:args forType:[op hasSuffix:@"Text"] ? NSPasteboardTypeString : NSPasteboardTypeHTML];
    } else if ([op isEqualToString:@"clipboard.clear"]) [[NSPasteboard generalPasteboard] clearContents];
    else if ([op isEqualToString:@"shell.openExternal"]) {
        NSURL *url = [NSURL URLWithString:args];
        if (!url || ![@[@"http", @"https", @"mailto", @"tel"] containsObject:url.scheme.lowercaseString]) return strdup("{\"error\":\"Unsupported external URL scheme\"}");
        result = @([[NSWorkspace sharedWorkspace] openURL:url]);
    } else if ([op isEqualToString:@"shell.openPath"]) result = @([[NSWorkspace sharedWorkspace] openURL:[NSURL fileURLWithPath:args]]);
    else if ([op isEqualToString:@"shell.showItemInFolder"]) {
        NSURL *url = [NSURL fileURLWithPath:args];
        if (!url) return strdup("{\"error\":\"Expected a file path\"}");
        [[NSWorkspace sharedWorkspace] activateFileViewerSelectingURLs:@[url]];
    }
    else if ([op isEqualToString:@"theme.get"]) {
        AppsNativeOwner();
        uint32_t state = apps_theme_state();
        result = @{ @"themeSource": AppsThemeSource, @"shouldUseDarkColors": @((BOOL)((state & 1) != 0)),
            @"shouldUseHighContrastColors": @((BOOL)((state & 2) != 0)), @"shouldUseInvertedColorScheme": @((BOOL)((state & 4) != 0)),
            @"shouldUseDarkColorsForSystemIntegratedUI": @((BOOL)((state & 1) != 0)), @"inForcedColorsMode": @NO,
            @"prefersReducedTransparency": @((BOOL)((state & 8) != 0)), @"shouldDifferentiateWithoutColor": @((BOOL)((state & 16) != 0)) };
    }
    else if ([op isEqualToString:@"theme.set"]) {
        AppsNative *owner = AppsNativeOwner();
        AppsThemeSource = [args copy];
        NSApp.appearance = [args isEqual:@"system"] ? nil : [NSAppearance appearanceNamed:[args isEqual:@"dark"] ? NSAppearanceNameDarkAqua : NSAppearanceNameAqua];
        [owner themeChanged:nil];
    }
    else if ([op hasPrefix:@"safeStorage."]) {
        OSStatus status;
        NSData *secret = AppsSecret(&status);
        if ([op isEqualToString:@"safeStorage.available"]) result = @(secret.length == 64);
        else {
            if (secret.length != 64) return strdup(AppsJSON(@{ @"error": [NSString stringWithFormat:@"Keychain access failed (%d)", (int)status] }).UTF8String);
            const unsigned char *key = secret.bytes;
            if ([op isEqualToString:@"safeStorage.encrypt"]) {
                NSData *plain = [args dataUsingEncoding:NSUTF8StringEncoding];
                NSMutableData *encrypted = [NSMutableData dataWithLength:20 + plain.length + kCCBlockSizeAES128];
                unsigned char *bytes = encrypted.mutableBytes;
                memcpy(bytes, "APPS", 4);
                if (SecRandomCopyBytes(kSecRandomDefault, 16, bytes + 4) != errSecSuccess) return strdup("{\"error\":\"Secure randomness failed\"}");
                size_t written = 0;
                CCCryptorStatus cryptStatus = CCCrypt(kCCEncrypt, kCCAlgorithmAES, kCCOptionPKCS7Padding, key, 32, bytes + 4, plain.bytes, plain.length, bytes + 20, encrypted.length - 20, &written);
                if (cryptStatus != kCCSuccess) return strdup("{\"error\":\"Encryption failed\"}");
                encrypted.length = 20 + written;
                unsigned char mac[CC_SHA256_DIGEST_LENGTH];
                CCHmac(kCCHmacAlgSHA256, key + 32, 32, encrypted.bytes, encrypted.length, mac);
                [encrypted appendBytes:mac length:sizeof(mac)];
                result = [encrypted base64EncodedStringWithOptions:0];
            } else if ([op isEqualToString:@"safeStorage.decrypt"]) {
                NSData *encrypted = [[NSData alloc] initWithBase64EncodedString:args options:0];
                const unsigned char *bytes = encrypted.bytes;
                if (encrypted.length < 68 || memcmp(bytes, "APPS", 4)) return strdup("{\"error\":\"Invalid encrypted data\"}");
                unsigned char mac[CC_SHA256_DIGEST_LENGTH];
                CCHmac(kCCHmacAlgSHA256, key + 32, 32, bytes, encrypted.length - sizeof(mac), mac);
                unsigned char difference = 0;
                for (size_t i = 0; i < sizeof(mac); i++) difference |= mac[i] ^ bytes[encrypted.length - sizeof(mac) + i];
                if (difference) return strdup("{\"error\":\"Encrypted data authentication failed\"}");
                NSMutableData *plain = [NSMutableData dataWithLength:encrypted.length];
                size_t written = 0;
                CCCryptorStatus cryptStatus = CCCrypt(kCCDecrypt, kCCAlgorithmAES, kCCOptionPKCS7Padding, key, 32, bytes + 4, bytes + 20, encrypted.length - 52, plain.mutableBytes, plain.length, &written);
                if (cryptStatus != kCCSuccess) return strdup("{\"error\":\"Decryption failed\"}");
                plain.length = written;
                result = [[NSString alloc] initWithData:plain encoding:NSUTF8StringEncoding];
                if (!result) return strdup("{\"error\":\"Decrypted data is not UTF-8\"}");
            } else return strdup("{\"error\":\"Unknown secure storage operation\"}");
        }
    }
    else if ([op isEqualToString:@"notification.show"]) {
        if (!NSBundle.mainBundle.bundleIdentifier) return strdup("{\"error\":\"Notifications require an app bundle identifier\"}");
        UNUserNotificationCenter *center = [UNUserNotificationCenter currentNotificationCenter];
        center.delegate = AppsNativeOwner();
        UNMutableNotificationContent *content = [UNMutableNotificationContent new];
        content.title = args[@"title"] ?: @"";
        content.body = args[@"body"] ?: @"";
        if (![args[@"silent"] boolValue]) content.sound = [UNNotificationSound defaultSound];
        NSString *identifier = args[@"id"] ?: NSUUID.UUID.UUIDString;
        [center requestAuthorizationWithOptions:UNAuthorizationOptionAlert | UNAuthorizationOptionSound | UNAuthorizationOptionBadge completionHandler:^(BOOL granted, NSError *error) {
            if (!granted || error) { AppsNativeEmit(@{ @"type": @"notification", @"id": identifier, @"event": @"failed", @"error": error.localizedDescription ?: @"Notification permission denied" }); return; }
            [center addNotificationRequest:[UNNotificationRequest requestWithIdentifier:identifier content:content trigger:nil] withCompletionHandler:^(NSError *failure) {
                AppsNativeEmit(failure ? @{ @"type": @"notification", @"id": identifier, @"event": @"failed", @"error": failure.localizedDescription } : @{ @"type": @"notification", @"id": identifier, @"event": @"show" });
            }];
        }];
        result = identifier;
    } else if ([op isEqualToString:@"notification.close"]) [[UNUserNotificationCenter currentNotificationCenter] removeDeliveredNotificationsWithIdentifiers:@[args]];
    else if ([op isEqualToString:@"screen.displays"]) {
        NSMutableArray *displays = [NSMutableArray new];
        CGFloat top = NSMaxY(NSScreen.screens.firstObject.frame);
        NSDictionary *(^rect)(NSRect) = ^NSDictionary *(NSRect frame) { return @{ @"x": @(frame.origin.x), @"y": @(top - NSMaxY(frame)), @"width": @(frame.size.width), @"height": @(frame.size.height) }; };
        for (NSScreen *screen in NSScreen.screens) {
            CGDirectDisplayID display = [screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue];
            NSInteger frequency = 0;
            if (@available(macOS 12.0, *)) frequency = screen.maximumFramesPerSecond;
            [displays addObject:@{ @"id": @(display), @"label": screen.localizedName ?: @"", @"bounds": rect(screen.frame), @"workArea": rect(screen.visibleFrame), @"size": @{ @"width": @(screen.frame.size.width), @"height": @(screen.frame.size.height) }, @"workAreaSize": @{ @"width": @(screen.visibleFrame.size.width), @"height": @(screen.visibleFrame.size.height) }, @"scaleFactor": @(screen.backingScaleFactor), @"rotation": @((NSInteger)CGDisplayRotation(display)), @"internal": @((BOOL)CGDisplayIsBuiltin(display)), @"monochrome": @NO, @"accelerometerSupport": @"unknown", @"touchSupport": @"unknown", @"colorDepth": @(NSBitsPerPixelFromDepth(screen.depth)), @"depthPerComponent": @(NSBitsPerSampleFromDepth(screen.depth)), @"colorSpace": screen.colorSpace.localizedName ?: @"", @"displayFrequency": @(frequency), @"detected": @YES }];
        }
        result = displays;
    } else if ([op isEqualToString:@"screen.cursor"]) {
        NSPoint point = NSEvent.mouseLocation;
        result = @{ @"x": @(floor(point.x)), @"y": @(floor(NSMaxY(NSScreen.screens.firstObject.frame) - point.y)) };
    }
    else if ([op isEqualToString:@"app.badge"]) NSApp.dockTile.badgeLabel = [args length] ? args : nil;
    else if ([op isEqualToString:@"app.locale"]) result = @{ @"system": [[NSLocale.currentLocale.localeIdentifier componentsSeparatedByString:@"@"][0] stringByReplacingOccurrencesOfString:@"_" withString:@"-"], @"languages": NSLocale.preferredLanguages };
    else if ([op isEqualToString:@"app.hidden"]) { if ([args boolValue]) [NSApp hide:nil]; else [NSApp unhide:nil]; }
    else if ([op isEqualToString:@"app.state"]) result = @{ @"hidden": @(NSApp.isHidden), @"active": @(NSApp.isActive) };
    else if ([op isEqualToString:@"app.attention"]) result = @([NSApp requestUserAttention:[args isEqual:@"critical"] ? NSCriticalRequest : NSInformationalRequest]);
    else if ([op isEqualToString:@"app.cancelAttention"]) [NSApp cancelUserAttentionRequest:[args integerValue]];
    else if ([op isEqualToString:@"app.downloadFinished"]) [[NSDistributedNotificationCenter defaultCenter] postNotificationName:@"com.apple.DownloadFileFinished" object:[args stringByResolvingSymlinksInPath]];
    else if ([op isEqualToString:@"app.activationPolicy"]) {
        NSArray *policies = @[@"regular", @"accessory", @"prohibited"];
        if (args != [NSNull null]) [NSApp setActivationPolicy:[policies indexOfObject:args]];
        result = policies[MIN(2, NSApp.activationPolicy)];
    } else if ([op isEqualToString:@"app.icon"]) {
        NSImage *image = [args length] ? [[NSImage alloc] initWithContentsOfFile:args] : nil;
        if ([args length] && !image) return strdup("{\"error\":\"Cannot load the icon image\"}");
        NSApp.applicationIconImage = image;
    } else if ([op isEqualToString:@"app.about"]) {
        if (args == [NSNull null]) [NSApp orderFrontStandardAboutPanelWithOptions:AppsAboutOptions ?: @{}];
        else {
            NSMutableDictionary *options = [NSMutableDictionary new];
            NSDictionary *keys = @{ @"applicationName": NSAboutPanelOptionApplicationName, @"applicationVersion": NSAboutPanelOptionApplicationVersion, @"version": NSAboutPanelOptionVersion, @"copyright": @"Copyright" };
            for (NSString *key in keys) if (args[key]) options[keys[key]] = args[key];
            if (args[@"credits"]) options[NSAboutPanelOptionCredits] = [[NSAttributedString alloc] initWithString:args[@"credits"]];
            if (args[@"iconPath"]) {
                NSImage *image = [[NSImage alloc] initWithContentsOfFile:args[@"iconPath"]];
                if (!image) return strdup("{\"error\":\"Cannot load the icon image\"}");
                options[NSAboutPanelOptionApplicationIcon] = image;
            }
            AppsAboutOptions = options;
        }
    } else if ([op isEqualToString:@"app.loginItem"]) {
        if (@available(macOS 13, *)) {
            SMAppService *service = SMAppService.mainAppService;
            if (args != [NSNull null] && [args boolValue] != (service.status == SMAppServiceStatusEnabled)) {
                if ([args boolValue]) [service registerAndReturnError:nil]; else [service unregisterAndReturnError:nil];
            }
            result = @{ @"openAtLogin": @((BOOL)(service.status == SMAppServiceStatusEnabled)), @"status": @[@"not-registered", @"enabled", @"requires-approval", @"not-found"][MIN(3, service.status)] };
        } else result = @{ @"openAtLogin": @NO, @"status": @"not-found" };
    }
    else if ([op hasPrefix:@"dialog."]) {
        // WARNING: async dialogs must not run a modal loop: the app stays borrowed and the main queue stops while it runs.
        NSNumber *request = args[@"async"];
        void (^emit)(NSDictionary *) = ^(NSDictionary *value) { AppsNativeEmit(@{ @"type": @"dialog", @"id": request, @"value": value }); };
        if ([op isEqualToString:@"dialog.open"] || [op isEqualToString:@"dialog.save"]) {
            BOOL open = [op isEqualToString:@"dialog.open"];
            NSSavePanel *panel = open ? [NSOpenPanel openPanel] : [NSSavePanel savePanel];
            panel.title = args[@"title"] ?: (open ? @"Open" : @"Save");
            if (args[@"buttonLabel"]) panel.prompt = args[@"buttonLabel"];
            if (args[@"message"]) panel.message = args[@"message"];
            if (args[@"nameFieldLabel"] && !open) panel.nameFieldLabel = args[@"nameFieldLabel"];
            NSMutableArray *types = [NSMutableArray new];
            BOOL any = NO;
            for (NSDictionary *filter in args[@"filters"] ?: @[]) for (NSString *extension in filter[@"extensions"]) {
                if ([extension isEqual:@"*"]) any = YES;
                else if (@available(macOS 11.0, *)) { UTType *type = [UTType typeWithFilenameExtension:extension]; if (type) [types addObject:type]; }
            }
            if (@available(macOS 11.0, *)) if (types.count && !any) panel.allowedContentTypes = types;
            if (open) {
                NSOpenPanel *chooser = (NSOpenPanel *)panel;
                NSArray *properties = args[@"properties"] ?: @[@"openFile"];
                chooser.canChooseDirectories = [properties containsObject:@"openDirectory"];
                chooser.canChooseFiles = [properties containsObject:@"openFile"] || !chooser.canChooseDirectories;
                chooser.allowsMultipleSelection = [properties containsObject:@"multiSelections"];
                chooser.canCreateDirectories = [properties containsObject:@"createDirectory"];
                if (args[@"defaultPath"]) chooser.directoryURL = [NSURL fileURLWithPath:args[@"defaultPath"]];
            } else if (args[@"defaultPath"]) {
                NSString *path = args[@"defaultPath"];
                panel.directoryURL = [NSURL fileURLWithPath:path.stringByDeletingLastPathComponent];
                panel.nameFieldStringValue = path.lastPathComponent;
            }
            NSDictionary *(^finish)(NSModalResponse) = ^NSDictionary *(NSModalResponse response) {
                BOOL ok = response == NSModalResponseOK;
                if (!open) return @{ @"canceled": @(!ok), @"filePath": ok ? panel.URL.path : [NSNull null] };
                NSMutableArray *paths = [NSMutableArray new];
                if (ok) for (NSURL *url in ((NSOpenPanel *)panel).URLs) [paths addObject:url.path];
                return @{ @"canceled": @(!ok), @"filePaths": paths };
            };
            if (request) [panel beginWithCompletionHandler:^(NSModalResponse response) { emit(finish(response)); }];
            else result = finish([panel runModal]);
        } else if ([op isEqualToString:@"dialog.message"]) {
            NSAlert *alert = [NSAlert new];
            alert.messageText = args[@"message"] ?: @"";
            alert.informativeText = args[@"detail"] ?: @"";
            alert.alertStyle = [args[@"type"] isEqual:@"warning"] || [args[@"type"] isEqual:@"error"] ? NSAlertStyleCritical : NSAlertStyleInformational;
            NSArray *buttons = [args[@"buttons"] count] ? args[@"buttons"] : @[@"OK"];
            NSInteger defaultId = args[@"defaultId"] ? [args[@"defaultId"] integerValue] : 0, cancelId = args[@"cancelId"] ? [args[@"cancelId"] integerValue] : -1;
            for (NSUInteger i = 0; i < buttons.count; i++) {
                NSButton *button = [alert addButtonWithTitle:buttons[i]];
                button.keyEquivalent = (NSInteger)i == defaultId ? @"\r" : (NSInteger)i == cancelId ? @"\033" : @"";
            }
            if (args[@"checkboxLabel"]) {
                alert.showsSuppressionButton = YES;
                alert.suppressionButton.title = args[@"checkboxLabel"];
                alert.suppressionButton.state = [args[@"checkboxChecked"] boolValue] ? NSControlStateValueOn : NSControlStateValueOff;
            }
            NSDictionary *(^finish)(NSModalResponse) = ^NSDictionary *(NSModalResponse response) {
                return @{ @"response": @(response - NSAlertFirstButtonReturn), @"checkboxChecked": @(alert.suppressionButton.state == NSControlStateValueOn) };
            };
            NSWindow *window = NSApp.keyWindow ?: NSApp.mainWindow ?: NSApp.orderedWindows.firstObject, *host = nil;
            if (request && !window) {
                // WARNING: an alert without a window has no non-modal form, so it hangs from a clear window.
                NSRect screen = NSScreen.mainScreen.visibleFrame;
                host = [[NSWindow alloc] initWithContentRect:NSMakeRect(NSMidX(screen) - 210, NSMidY(screen) + 120, 420, 1) styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
                host.opaque = NO;
                host.backgroundColor = NSColor.clearColor;
                host.releasedWhenClosed = NO;
                host.level = NSModalPanelWindowLevel;
                [host orderFrontRegardless];
                window = host;
            }
            if (request) [alert beginSheetModalForWindow:window completionHandler:^(NSModalResponse response) { emit(finish(response)); [host orderOut:nil]; }];
            else result = finish([alert runModal]);
        } else return strdup("{\"error\":\"Unknown native operation\"}");
    } else return strdup("{\"error\":\"Unknown native operation\"}");
    return strdup(AppsJSON(@{ @"value": result }).UTF8String);
}

void apps_native_free(char *text) { free(text); }
