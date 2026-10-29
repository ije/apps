#import <AppKit/AppKit.h>
#import <Carbon/Carbon.h>
#import <objc/runtime.h>
#include <string.h>
char *apps_native_call(const char *,const char *);
void apps_native_free(char *);
void apps_theme_observe(void (*)(void));
static int themeWakes;
static void themeWake(void) { themeWakes++; }
static BOOL preferenceEnabled;
static BOOL preference(id object, SEL selector) { return preferenceEnabled; }
@interface GPUIView : NSView
@property(nonatomic, strong) NSEvent *key;
@end
@implementation GPUIView
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
        GPUIView *view = [[GPUIView alloc] initWithFrame:window.contentView.bounds];
        window.contentView = view;
        [window makeFirstResponder:view];
        if (![NSApp sendAction:copy.action to:copy.target from:copy] || view.key.keyCode != kVK_ANSI_C || view.key.modifierFlags != NSEventModifierFlagCommand) return 16;
        NSMenuItem *devtools = [[menu itemWithTitle:@"View"].submenu itemWithTitle:@"Toggle Developer Tools"];
        if (![NSApp sendAction:devtools.action to:devtools.target from:devtools] || view.key.keyCode != kVK_ANSI_I || view.key.modifierFlags != (NSEventModifierFlagCommand | NSEventModifierFlagOption)) return 17;
        [window close];
        native("menu.set", "[{\"label\":\"Custom\",\"submenu\":[{\"label\":\"Copy\",\"role\":\"copy\"}]}]");
        if (NSApp.mainMenu.numberOfItems != 1 || ![[NSApp.mainMenu itemAtIndex:0].title isEqual:@"Custom"]) return 18;
        native("menu.set", "null");
        if (NSApp.mainMenu) return 19;
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
        if (!received || task.terminationStatus || [native("app.hasInstance","null") boolValue]) return 9;
        native("app.badge","\"3\"");
        if (![NSApp.dockTile.badgeLabel isEqual:@"3"]) return 10;
        native("app.badge","\"\"");
        if (NSApp.dockTile.badgeLabel || ![native("app.activationPolicy","null") isEqual:@"accessory"]) return 10;
        NSDictionary *locale=native("app.locale","null");
        if (![locale[@"system"] length] || ![locale[@"languages"] isKindOfClass:NSArray.class]) return 11;
        native("app.about","{\"applicationName\":\"Smoke\",\"credits\":\"Thanks\"}");
        if ([[menu itemAtIndex:0].submenu itemAtIndex:0].action != @selector(about:)) return 28;
        return 0;
    }
}
