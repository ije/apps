#import <AppKit/AppKit.h>
#import <Carbon/Carbon.h>
#import <IOKit/pwr_mgt/IOPMLib.h>
#import <objc/runtime.h>
#include <string.h>
#include <unistd.h>
char *apps_native_call(const char *,const char *);
void apps_native_free(char *);
void apps_theme_observe(void (*)(void));
static int themeWakes;
static void themeWake(void) { themeWakes++; }
static BOOL preferenceEnabled;
static BOOL preference(id object, SEL selector) { return preferenceEnabled; }
@interface AppsView : NSView
@property(nonatomic, strong) NSEvent *key;
@end
@implementation AppsView
- (BOOL)acceptsFirstResponder { return YES; }
- (BOOL)performKeyEquivalent:(NSEvent *)event { self.key = event; return YES; }
@end
static id native(const char *operation, const char *arguments) {
    char *reply=apps_native_call(operation,arguments);
    NSDictionary *value=[NSJSONSerialization JSONObjectWithData:[[NSString stringWithUTF8String:reply] dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
    apps_native_free(reply);
    return value[@"value"];
}
int main(int argc, const char *argv[]) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
        if (argc==3 && !strcmp(argv[1],"--second-instance")) {
            BOOL owns=[native("app.singleInstance",argv[2]) boolValue];
            [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.1]];
            return owns?5:0;
        }
        if (argc != 2) return 12;
        [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        [NSApp finishLaunching];
        apps_theme_observe(themeWake);
        if (!strcmp(argv[1], "--tray")) {
            alarm(8);
            __block NSStatusItem *status;
            Method method = class_getInstanceMethod(NSStatusBar.class, @selector(statusItemWithLength:));
            IMP original = method_getImplementation(method);
            IMP capture = imp_implementationWithBlock(^NSStatusItem *(NSStatusBar *bar, CGFloat length) {
                status = ((NSStatusItem *(*)(id, SEL, CGFloat))original)(bar, @selector(statusItemWithLength:), length);
                return status;
            });
            method_setImplementation(method, capture);
            NSString *icon = [[NSString stringWithUTF8String:argv[0]].stringByDeletingLastPathComponent stringByAppendingPathComponent:@"ServerTemplate.tiff"];
            [[NSImage imageNamed:NSImageNameStatusAvailable].TIFFRepresentation writeToFile:icon atomically:YES];
            NSDictionary *create = @{ @"id": @1, @"image": icon, @"guid": @"a64ad871-6826-4a77-90f9-900e39d536dd" };
            NSData *json = [NSJSONSerialization dataWithJSONObject:create options:0 error:nil];
            native("tray.create", [[NSString alloc] initWithData:json encoding:NSUTF8StringEncoding].UTF8String);
            method_setImplementation(method, original);
            imp_removeBlock(capture);
            if (!status.button.image.isTemplate || ![status.autosaveName isEqual:create[@"guid"]]) return 34;
            native("tray.title", "{\"id\":1,\"text\":\"Server\"}");
            native("tray.toolTip", "{\"id\":1,\"text\":\"Running\"}");
            NSView *view = status.button.subviews.lastObject;
            if (![native("tray.getTitle", "{\"id\":1}") isEqual:@"Server"] || ![view.toolTip isEqual:@"Running"]) return 35;
            if ([native("tray.bounds", "{\"id\":1}")[@"width"] doubleValue] <= 0) return 36;
            NSEvent *click = [NSEvent mouseEventWithType:NSEventTypeLeftMouseUp location:NSZeroPoint modifierFlags:NSEventModifierFlagOption timestamp:0 windowNumber:view.window.windowNumber context:nil eventNumber:0 clickCount:1 pressure:0];
            [view mouseUp:click];
            [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.02]];
            NSArray *events = native("poll", "null");
            if (![events filteredArrayUsingPredicate:[NSPredicate predicateWithFormat:@"event == 'click' AND id == 1 AND alt == YES"]].count) return 37;
            native("tray.menu", "{\"id\":1,\"template\":[{\"label\":\"Server\",\"sublabel\":\"Running\",\"submenu\":[{\"label\":\"Settings\",\"click\":42}]},{\"type\":\"separator\"},{\"role\":\"quit\",\"label\":\"Quit\"}]}");
            __block BOOL selected = NO;
            id observer = [NSNotificationCenter.defaultCenter addObserverForName:NSMenuDidBeginTrackingNotification object:nil queue:nil usingBlock:^(NSNotification *note) {
                NSMenu *menu = note.object;
                if (menu != status.menu) return;
                [NSRunLoop.mainRunLoop performInModes:@[NSEventTrackingRunLoopMode, NSDefaultRunLoopMode] block:^{
                    NSMenuItem *row = menu.itemArray.firstObject;
                    selected = [row.title isEqual:@"Server"] && [menu itemAtIndex:1].isSeparatorItem && [menu itemAtIndex:2].action == @selector(terminate:);
                    if (@available(macOS 14.4, *)) selected &= [row.subtitle isEqual:@"Running"];
                    [row.submenu performActionForItemAtIndex:0];
                    native("tray.close", "{\"id\":1}");
                }];
            }];
            native("tray.popup", "{\"id\":1,\"template\":null,\"request\":3}");
            [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.4]];
            [NSNotificationCenter.defaultCenter removeObserver:observer];
            events = native("poll", "null");
            if (!selected || ![events filteredArrayUsingPredicate:[NSPredicate predicateWithFormat:@"type == 'menu' AND id == 42"]].count) return 38;
            native("tray.destroy", "{\"id\":1}");
            if (view.superview || native("tray.bounds", "{\"id\":1}")) return 39;
            return 0;
        }
        native("theme.set", "\"dark\"");
        NSDictionary *theme = native("theme.get", "null");
        for (NSString *key in theme) if (![key isEqual:@"themeSource"] && CFGetTypeID((__bridge CFTypeRef)theme[key]) != CFBooleanGetTypeID()) return 27;
        if (![theme[@"shouldUseDarkColors"] boolValue] || ![theme[@"shouldUseDarkColorsForSystemIntegratedUI"] boolValue] || ![theme[@"themeSource"] isEqual:@"dark"]) return 20;
        if (![[NSApp.effectiveAppearance bestMatchFromAppearancesWithNames:@[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]] isEqual:NSAppearanceNameDarkAqua]) return 21;
        [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.02]];
        native("poll", "null"); themeWakes = 0;
        native("theme.set", "\"dark\"");
        [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.02]];
        if (themeWakes || [native("poll", "null") count]) return 22;
        native("theme.set", "\"light\"");
        [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.02]];
        if ([native("theme.get", "null")[@"shouldUseDarkColors"] boolValue] || themeWakes != 1 || [native("poll", "null") count] != 1) return 23;
        native("theme.set", "\"system\"");
        theme = native("theme.get", "null");
        if (NSApp.appearance || ![theme[@"themeSource"] isEqual:@"system"] || [theme[@"inForcedColorsMode"] boolValue]) return 24;
        NSDictionary *preferences = @{ @"shouldUseHighContrastColors": @"accessibilityDisplayShouldIncreaseContrast", @"shouldUseInvertedColorScheme": @"accessibilityDisplayShouldInvertColors", @"prefersReducedTransparency": @"accessibilityDisplayShouldReduceTransparency", @"shouldDifferentiateWithoutColor": @"accessibilityDisplayShouldDifferentiateWithoutColor" };
        for (NSString *key in preferences) {
            Method method = class_getInstanceMethod(NSWorkspace.class, NSSelectorFromString(preferences[key]));
            IMP original = method_setImplementation(method, (IMP)preference);
            for (NSNumber *enabled in @[@NO, @YES, @NO]) {
                preferenceEnabled = enabled.boolValue;
                [NSWorkspace.sharedWorkspace.notificationCenter postNotificationName:NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification object:nil];
                [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.02]];
                if (![native("theme.get", "null")[key] isEqual:enabled]) return 25;
                if (enabled.boolValue && ![native("poll", "null") count]) return 26;
            }
            method_setImplementation(method, original);
        }
        native("poll", "null");
        [NSApp activateIgnoringOtherApps:YES];
        NSString *template = [NSString stringWithContentsOfFile:[NSString stringWithUTF8String:argv[1]] encoding:NSUTF8StringEncoding error:nil];
        native("menu.set", template.UTF8String);
        NSMenu *menu = NSApp.mainMenu;
        if (menu.numberOfItems != 6 || ![[menu itemAtIndex:0].submenu.title isEqual:@"Menu Test"] || !NSApp.servicesMenu || !NSApp.windowsMenu || !NSApp.helpMenu) return 13;
        NSMenuItem *quit = [[menu itemAtIndex:0].submenu itemWithTitle:@"Quit Menu Test"];
        if (!quit || quit.action != @selector(terminate:) || ![quit.keyEquivalent isEqual:@"q"] || quit.keyEquivalentModifierMask != NSEventModifierFlagCommand) return 14;
        NSMenuItem *copy = [[menu itemWithTitle:@"Edit"].submenu itemWithTitle:@"Copy"];
        NSMenuItem *select = [[menu itemWithTitle:@"Edit"].submenu itemWithTitle:@"Select All"];
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,200,100) styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        window.releasedWhenClosed = NO;
        NSTextView *text = [[NSTextView alloc] initWithFrame:window.contentView.bounds];
        text.string = @"native selection";
        window.contentView = text;
        [window makeKeyAndOrderFront:nil];
        [window makeFirstResponder:text];
        NSDate *activationDeadline = [NSDate dateWithTimeIntervalSinceNow:1];
        while (!NSApp.active && activationDeadline.timeIntervalSinceNow > 0) {
            NSEvent *event = [NSApp nextEventMatchingMask:NSEventMaskAny untilDate:activationDeadline inMode:NSDefaultRunLoopMode dequeue:YES];
            if (event) [NSApp sendEvent:event];
        }
        [window makeKeyWindow];
        if (![NSApp sendAction:select.action to:select.target from:select] || text.selectedRange.length != text.string.length) return 15;
        // The page view is a subview of the content view, as in a real window.
        AppsView *view = [[AppsView alloc] initWithFrame:window.contentView.bounds];
        window.contentView = [[NSView alloc] initWithFrame:view.frame];
        [window.contentView addSubview:view];
        [window makeFirstResponder:view];
        if (![NSApp sendAction:copy.action to:copy.target from:copy] || view.key.keyCode != kVK_ANSI_C || view.key.modifierFlags != NSEventModifierFlagCommand) return 16;
        NSMenuItem *devtools = [[menu itemWithTitle:@"View"].submenu itemWithTitle:@"Toggle Developer Tools"];
        [window makeFirstResponder:nil];
        if (![NSApp sendAction:devtools.action to:devtools.target from:devtools] || view.key.keyCode != kVK_ANSI_I || view.key.modifierFlags != (NSEventModifierFlagCommand | NSEventModifierFlagOption)) return 17;
        [window close];
        native("menu.set", "[{\"label\":\"Custom\",\"submenu\":[{\"label\":\"Copy\",\"role\":\"copy\"}]}]");
        if (NSApp.mainMenu.numberOfItems != 1 || ![[NSApp.mainMenu itemAtIndex:0].title isEqual:@"Custom"]) return 18;
        native("menu.set", "null");
        if (NSApp.mainMenu) return 19;
        native("menu.set", "[{\"label\":\"Keys\",\"submenu\":[{\"label\":\"Home\",\"accelerator\":\"Meta+Home\"},{\"label\":\"F13\",\"accelerator\":\"F13\"},{\"label\":\"Plus\",\"accelerator\":\"CmdOrCtrl+Plus\"},{\"label\":\"Slash\",\"accelerator\":\"Cmd+\\\\\"},{\"label\":\"Media\",\"accelerator\":\"MediaPlayPause\"},{\"label\":\"Zoom\",\"role\":\"zoomin\"},{\"label\":\"A\",\"type\":\"radio\",\"checked\":true,\"click\":1},{\"label\":\"B\",\"type\":\"radio\",\"click\":2}]}]");
        NSMenu *keys = [NSApp.mainMenu itemAtIndex:0].submenu;
        unichar home = NSHomeFunctionKey, f13 = NSF13FunctionKey;
        if (keys.numberOfItems != 8 || ![[keys itemAtIndex:0].keyEquivalent isEqual:[NSString stringWithCharacters:&home length:1]] || [keys itemAtIndex:0].keyEquivalentModifierMask != NSEventModifierFlagCommand || ![[keys itemAtIndex:1].keyEquivalent isEqual:[NSString stringWithCharacters:&f13 length:1]] || ![[keys itemAtIndex:2].keyEquivalent isEqual:@"+"] || ![[keys itemAtIndex:3].keyEquivalent isEqual:@"\\"] || [keys itemAtIndex:4].keyEquivalent.length || [keys itemAtIndex:5].action != @selector(menuAction:)) return 31;
        [NSApp sendAction:[keys itemAtIndex:7].action to:[keys itemAtIndex:7].target from:[keys itemAtIndex:7]];
        if ([keys itemAtIndex:6].state != NSControlStateValueOff || [keys itemAtIndex:7].state != NSControlStateValueOn) return 32;
        native("poll", "null");
        native("menu.set", "null");
        NSArray *displays = native("screen.displays", "null");
        if (!displays.count || [displays[0][@"bounds"][@"x"] doubleValue] != 0 || [displays[0][@"bounds"][@"y"] doubleValue] != 0 || [displays[0][@"scaleFactor"] doubleValue] < 1 || ![native("screen.cursor", "null")[@"x"] isKindOfClass:NSNumber.class]) return 33;
        NSNumber *assertion = native("power.block", "true");
        CFDictionaryRef properties = IOPMAssertionCopyProperties(assertion.unsignedIntValue);
        if (!properties) return 34;
        CFRelease(properties);
        native("power.unblock", assertion.stringValue.UTF8String);
        properties = IOPMAssertionCopyProperties(assertion.unsignedIntValue);
        if (properties) { CFRelease(properties); return 34; }
        NSString *accent = native("prefs.color", "\"controlAccentColor\"");
        if (accent.length != 9 || ![accent hasPrefix:@"#"] || [native("prefs.color", "\"systemBlueColor\"") length] != 9 || native("prefs.color", "\"missingColor\"") != [NSNull null]) return 35;
        if (![@[@"dark", @"light"] containsObject:native("prefs.appearance", "null")] || [native("prefs.animation", "null") count] != 3 || ![native("prefs.swipe", "null") isKindOfClass:NSNumber.class] || ![native("prefs.touchID", "null") isKindOfClass:NSNumber.class] || ![native("prefs.trusted", "false") isKindOfClass:NSNumber.class]) return 36;
        if (![@[@"not-determined", @"restricted", @"denied", @"granted"] containsObject:native("prefs.media", "\"camera\"")] || ![@[@"denied", @"granted"] containsObject:native("prefs.media", "\"screen\"")] || native("prefs.askMedia", "{\"type\":\"camera\",\"async\":1}")) return 37;
        NSString *key = [@"apps-test-" stringByAppendingString:NSUUID.UUID.UUIDString];
        NSDictionary *values = @{ @"string": @"text", @"boolean": @YES, @"integer": @7, @"double": @1.5, @"url": @"https://example.com/a", @"array": @[@1, @"two"], @"dictionary": @{ @"a": @1 } };
        for (NSString *type in values) {
            NSString *request = [[NSString alloc] initWithData:[NSJSONSerialization dataWithJSONObject:@{ @"key": key, @"type": type, @"value": values[type] } options:0 error:nil] encoding:NSUTF8StringEncoding];
            if (native("prefs.setDefault", request.UTF8String) != [NSNull null] || ![native("prefs.getDefault", request.UTF8String) isEqual:values[type]]) return 38;
        }
        NSString *quoted = [NSString stringWithFormat:@"\"%@\"", key], *string = [NSString stringWithFormat:@"{\"key\":\"%@\",\"type\":\"string\"}", key];
        native("prefs.removeDefault", quoted.UTF8String);
        if (native("prefs.setDefault", [NSString stringWithFormat:@"{\"key\":\"%@\",\"type\":\"array\",\"value\":[null]}", key].UTF8String) || ![native("prefs.getDefault", string.UTF8String) isEqual:@""]) return 39;
        native("prefs.registerDefaults", [NSString stringWithFormat:@"{\"%@\":\"registered\"}", key].UTF8String);
        if (![native("prefs.getDefault", string.UTF8String) isEqual:@"registered"]) return 40;
        NSDictionary *options=@{ @"id": NSUUID.UUID.UUIDString, @"argv": @[@"smoke",@"second"], @"cwd": NSTemporaryDirectory(), @"data": @{ @"value": @42 } };
        NSString *arguments=[[NSString alloc] initWithData:[NSJSONSerialization dataWithJSONObject:options options:0 error:nil] encoding:NSUTF8StringEncoding];
        if (![native("app.singleInstance",arguments.UTF8String) boolValue]) return 7;
        NSTask *task=[NSTask new];
        task.executableURL=[NSURL fileURLWithPath:[NSString stringWithUTF8String:argv[0]]];
        task.arguments=@[@"--second-instance",arguments];
        NSError *failure=nil;
        if (![task launchAndReturnError:&failure]) return 8;
        BOOL received=NO;
        NSDate *deadline=[NSDate dateWithTimeIntervalSinceNow:5];
        while (!received && deadline.timeIntervalSinceNow>0) {
            [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.02]];
            for (NSDictionary *message in native("poll","null")) if ([message[@"event"] isEqual:@"second-instance"] && [message[@"data"][@"value"] intValue]==42 && [message[@"args"] isEqual:options[@"argv"]]) received=YES;
        }
        [task waitUntilExit];
        native("app.releaseInstance","null");
        NSTask *again=[NSTask new];
        again.executableURL=task.executableURL;
        again.arguments=task.arguments;
        if (!received || task.terminationStatus || ![again launchAndReturnError:&failure]) return 9;
        [again waitUntilExit];
        if (again.terminationStatus!=5) return 9;
        native("app.badge","\"3\"");
        if (![NSApp.dockTile.badgeLabel isEqual:@"3"]) return 10;
        native("app.badge","\"\"");
        if (NSApp.dockTile.badgeLabel || ![native("app.activationPolicy","null") isEqual:@"accessory"]) return 10;
        native("app.progress","0.5");
        native("app.progress","2");
        NSView *bar = NSApp.dockTile.contentView.subviews.lastObject;
        if (![bar isKindOfClass:NSClassFromString(@"AppsDockProgress")] || [[bar valueForKey:@"value"] doubleValue] != 1) return 34;
        native("app.progress","-1");
        if (NSApp.dockTile.contentView) return 34;
        NSDictionary *locale=native("app.locale","null");
        if (![locale[@"system"] length] || ![locale[@"languages"] isKindOfClass:NSArray.class]) return 11;
        native("app.about","{\"applicationName\":\"Smoke\",\"credits\":\"Thanks\"}");
        if ([[menu itemAtIndex:0].submenu itemAtIndex:0].action != @selector(about:)) return 28;
        [native("poll", "null") count];
        NSWindow *owner = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 200, 100) styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        [owner orderFront:nil];
        for (NSString *request in @[@"{\"message\":\"Async\",\"type\":\"warning\",\"buttons\":[\"OK\",\"Cancel\"],\"defaultId\":1,\"cancelId\":0,\"async\":7}", @"{\"message\":\"Prompt\",\"buttons\":[\"OK\",\"Cancel\"],\"cancelId\":1,\"input\":\"typed\",\"async\":9}", @"{\"filters\":[{\"name\":\"Images\",\"extensions\":[\"png\",\"jpg\"]}],\"buttonLabel\":\"Pick\",\"async\":8}"]) {
            __block BOOL running = NO;
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 300 * NSEC_PER_MSEC), dispatch_get_main_queue(), ^{
                running = !NSApp.modalWindow;
                if (owner.attachedSheet) [owner endSheet:owner.attachedSheet returnCode:NSAlertSecondButtonReturn];
                for (NSWindow *panel in NSApp.windows) if ([panel isKindOfClass:NSSavePanel.class] && panel.visible) [(NSSavePanel *)panel cancel:nil];
            });
            if (native([request containsString:@"message"] ? "dialog.message" : "dialog.open", request.UTF8String) != [NSNull null]) return 29;
            NSArray *events = @[];
            for (int i = 0; i < 300 && !events.count; i++) {
                [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
                events = native("poll", "null");
            }
            if (!running || events.count != 1 || ![events[0][@"type"] isEqual:@"reply"] || !events[0][@"value"]) return 30;
            if ([request containsString:@"input"] && ![events[0][@"value"][@"input"] isEqual:@"typed"]) return 31;
        }
        return 0;
    }
}
