#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>
#import <CommonCrypto/CommonCrypto.h>
#include <stdlib.h>
#include <string.h>

typedef void (*AppsCallback)(void *, const char *);

@interface AppsBrowser : NSObject <WKNavigationDelegate, WKScriptMessageHandler, WKUIDelegate, NSWindowDelegate>
@property(nonatomic, strong) WKWebView *view;
@property(nonatomic, strong) NSWindow *window;
@property(nonatomic) AppsCallback callback;
@property(nonatomic) void *context;
@property(nonatomic) BOOL closed;
@property(nonatomic) BOOL visible;
@property(nonatomic) BOOL gated;
@property(nonatomic) BOOL programmatic;
@property(nonatomic) NSInteger nextDecision;
@property(nonatomic, strong) NSMutableDictionary *decisions;
@end

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
static void AppsEmit(AppsBrowser *owner, NSDictionary *event) {
    if (!owner.callback) return;
    NSData *data = [NSJSONSerialization isValidJSONObject:event] ? [NSJSONSerialization dataWithJSONObject:event options:0 error:nil] : nil;
    if (!data || data.length > 4 * 1024 * 1024) {
        if (![event[@"type"] isEqual:@"evaluation"]) return;
        data = [NSJSONSerialization dataWithJSONObject:@{ @"type": @"evaluation", @"id": event[@"id"], @"error": data ? @"Browser result exceeds 4 MiB" : @"Browser result cannot be serialized" } options:0 error:nil];
    }
    owner.callback(owner.context, [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding].UTF8String);
}

static WKWebsiteDataStore *AppsDataStore(NSString *partition) {
    if (!partition.length) return [WKWebsiteDataStore nonPersistentDataStore];
    static NSMutableDictionary *stores;
    static dispatch_once_t once;
    dispatch_once(&once, ^{ stores = [NSMutableDictionary new]; });
    WKWebsiteDataStore *store = stores[partition];
    if (store) return store;
    if ([partition isEqual:@"persist:default"]) store = [WKWebsiteDataStore defaultDataStore];
    else if ([partition hasPrefix:@"persist:"]) {
        if (@available(macOS 14.0, *)) {
            NSData *name = [partition dataUsingEncoding:NSUTF8StringEncoding];
            unsigned char digest[CC_SHA256_DIGEST_LENGTH];
            CCHmac(kCCHmacAlgSHA256, "apps-webkit-partition", sizeof("apps-webkit-partition"), name.bytes, name.length, digest);
            store = [WKWebsiteDataStore dataStoreForIdentifier:[[NSUUID alloc] initWithUUIDBytes:digest]];
        } else return nil;
    } else store = [WKWebsiteDataStore nonPersistentDataStore];
    stores[partition] = store;
    return store;
}

@implementation AppsBrowser
- (void)windowWillClose:(NSNotification *)notification { self.closed = YES; AppsEmit(self, @{ @"type": @"closed" }); }
- (void)userContentController:(WKUserContentController *)controller didReceiveScriptMessage:(WKScriptMessage *)message {
    if (![message.body isKindOfClass:[NSDictionary class]]) return;
    if ([message.body[@"kind"] isEqual:@"dom-ready"]) { if (message.frameInfo.mainFrame) AppsEmit(self, @{ @"type": @"dom-ready" }); return; }
    AppsEmit(self, @{ @"type": @"message", @"data": message.body, @"url": message.frameInfo.request.URL.absoluteString ?: @"", @"mainFrame": @(message.frameInfo.mainFrame) });
}
- (void)webView:(WKWebView *)view decidePolicyForNavigationAction:(WKNavigationAction *)action decisionHandler:(void (^)(WKNavigationActionPolicy))decisionHandler {
    if (!self.gated || self.programmatic || !action.targetFrame.isMainFrame || action.navigationType == WKNavigationTypeBackForward || action.navigationType == WKNavigationTypeReload || !self.callback) { decisionHandler(WKNavigationActionPolicyAllow); return; }
    NSNumber *decision = @(++self.nextDecision);
    self.decisions[decision] = [decisionHandler copy];
    AppsEmit(self, @{ @"type": @"will-navigate", @"id": decision, @"url": action.request.URL.absoluteString ?: @"" });
}
- (void)webView:(WKWebView *)view didStartProvisionalNavigation:(WKNavigation *)navigation {
    AppsEmit(self, @{ @"type": @"did-start-loading", @"url": view.URL.absoluteString ?: @"" });
}
- (void)webView:(WKWebView *)view didCommitNavigation:(WKNavigation *)navigation {
    self.programmatic = NO;
}
- (void)webView:(WKWebView *)view didFinishNavigation:(WKNavigation *)navigation {
    AppsEmit(self, @{ @"type": @"did-finish-load", @"url": view.URL.absoluteString ?: @"", @"title": view.title ?: @"", @"canGoBack": @(view.canGoBack), @"canGoForward": @(view.canGoForward) });
}
- (void)webView:(WKWebView *)view didFailNavigation:(WKNavigation *)navigation withError:(NSError *)error {
    AppsEmit(self, @{ @"type": @"did-fail-load", @"error": error.localizedDescription, @"code": @(error.code) });
}
- (void)webView:(WKWebView *)view didFailProvisionalNavigation:(WKNavigation *)navigation withError:(NSError *)error {
    self.programmatic = NO;
    [self webView:view didFailNavigation:navigation withError:error];
}
- (void)webViewWebContentProcessDidTerminate:(WKWebView *)view {
    AppsEmit(self, @{ @"type": @"render-process-gone" });
}
- (WKWebView *)webView:(WKWebView *)view createWebViewWithConfiguration:(WKWebViewConfiguration *)configuration forNavigationAction:(WKNavigationAction *)action windowFeatures:(WKWindowFeatures *)features {
    AppsEmit(self, @{ @"type": @"window-open", @"url": action.request.URL.absoluteString ?: @"", @"frameName": action.targetFrame ? @"" : @"_blank" });
    return nil;
}
@end

void *apps_browser_create(void *parent, const char *optionsJSON, AppsCallback callback, void *context) {
    if (!NSApp || ![NSThread isMainThread]) return NULL;
    NSDictionary *options = AppsValue(optionsJSON) ?: @{};
    AppsBrowser *owner = [AppsBrowser new];
    owner.visible = YES;
    owner.gated = [options[@"navigation"] boolValue];
    owner.decisions = [NSMutableDictionary new];
    owner.callback = callback;
    owner.context = context;
    WKWebViewConfiguration *configuration = [WKWebViewConfiguration new];
    WKWebsiteDataStore *store = AppsDataStore(options[@"partition"] ?: ([options[@"persistent"] boolValue] ? @"persist:default" : @""));
    if (!store) return NULL;
    configuration.websiteDataStore = store;
    WKWebpagePreferences *preferences = [WKWebpagePreferences new];
    preferences.allowsContentJavaScript = options[@"javascript"] ? [options[@"javascript"] boolValue] : YES;
    configuration.defaultWebpagePreferences = preferences;
    [configuration.userContentController addScriptMessageHandler:owner name:@"apps"];
    NSString *source = @"(()=>{const send=data=>window.webkit.messageHandlers.apps.postMessage(data);if(window===window.top)addEventListener('DOMContentLoaded',()=>send({kind:'dom-ready'}),{once:true});Object.defineProperty(window,'apps',{value:Object.freeze({postMessage:data=>send({kind:'message',data})})});for(const level of ['log','info','warn','error','debug']){const original=console[level];console[level]=(...args)=>{original.apply(console,args);send({kind:'console',level,args:args.map(v=>{try{return typeof v==='string'?v:JSON.stringify(v)}catch{return String(v)}})})}};addEventListener('error',e=>send({kind:'error',message:e.message,source:e.filename,line:e.lineno}));addEventListener('unhandledrejection',e=>send({kind:'error',message:String(e.reason)}));})();";
    [configuration.userContentController addUserScript:[[WKUserScript alloc] initWithSource:source injectionTime:WKUserScriptInjectionTimeAtDocumentStart forMainFrameOnly:NO]];
    owner.view = [[WKWebView alloc] initWithFrame:NSMakeRect(0, 0, 800, 600) configuration:configuration];
    owner.view.navigationDelegate = owner;
    owner.view.UIDelegate = owner;
    if (@available(macOS 13.3, *)) owner.view.inspectable = [options[@"inspectable"] boolValue];
    if (parent) {
        [(__bridge NSView *)parent addSubview:owner.view];
    } else {
        owner.window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 900, 650) styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskResizable | NSWindowStyleMaskMiniaturizable backing:NSBackingStoreBuffered defer:NO];
        owner.window.releasedWhenClosed = NO;
        owner.window.delegate = owner;
        owner.window.title = options[@"title"] ?: @"Browser";
        owner.window.contentView = owner.view;
        [owner.window center];
        if (!options[@"show"] || [options[@"show"] boolValue]) [owner.window makeKeyAndOrderFront:nil];
    }
    return (__bridge_retained void *)owner;
}

void apps_browser_destroy(void *handle) {
    if (!handle) return;
    AppsBrowser *owner = (__bridge_transfer AppsBrowser *)handle;
    owner.callback = NULL;
    owner.context = NULL;
    for (void (^decide)(WKNavigationActionPolicy) in owner.decisions.allValues) decide(WKNavigationActionPolicyCancel);
    [owner.decisions removeAllObjects];
    [owner.view stopLoading];
    owner.view.navigationDelegate = nil;
    owner.view.UIDelegate = nil;
    [owner.view.configuration.userContentController removeScriptMessageHandlerForName:@"apps"];
    [owner.view removeFromSuperview];
    [owner.window close];
}

void apps_browser_attach(void *handle, void *parent) {
    AppsBrowser *owner = (__bridge AppsBrowser *)handle;
    NSView *previous = owner.view.superview, *next = (__bridge NSView *)parent;
    NSRect frame = owner.view.frame;
    double top = previous && !previous.isFlipped ? previous.bounds.size.height - NSMaxY(frame) : frame.origin.y;
    [owner.view removeFromSuperview];
    [next addSubview:owner.view];
    frame.origin.y = !next || next.isFlipped ? top : next.bounds.size.height - top - frame.size.height;
    owner.view.frame = frame;
    owner.window.delegate = nil;
    [owner.window close];
    owner.window = nil;
}

void apps_browser_bounds(void *handle, double x, double y, double width, double height) {
    AppsBrowser *owner = (__bridge AppsBrowser *)handle;
    NSView *parent = owner.view.superview;
    owner.view.hidden = !owner.visible || width <= 0 || height <= 0;
    owner.view.frame = NSMakeRect(x, parent && !parent.isFlipped ? parent.bounds.size.height - y - height : y, MAX(0, width), MAX(0, height));
}

char *apps_browser_command(void *handle, const char *operation, const char *argumentsJSON) {
    // WARNING: completion blocks must not retain the owner; WebKit keeps unsettled callbacks, which would leak the view and its web process.
    __weak AppsBrowser *owner = (__bridge AppsBrowser *)handle;
    NSString *op = AppsString(operation);
    id args = AppsValue(argumentsJSON);
    WKWebView *view = owner.view;
    id result = [NSNull null];
    if ([op isEqualToString:@"loadURL"]) {
        NSURL *url = [NSURL URLWithString:args];
        if (!url || ![@[@"https", @"http", @"file", @"about", @"data"] containsObject:url.scheme.lowercaseString]) return strdup("{\"error\":\"Unsupported browser URL scheme\"}");
        owner.programmatic = YES;
        if (url.isFileURL) [view loadFileURL:url allowingReadAccessToURL:[url URLByDeletingLastPathComponent]];
        else [view loadRequest:[NSURLRequest requestWithURL:url]];
    } else if ([op isEqualToString:@"loadHTML"]) {
        owner.programmatic = YES;
        [view loadHTMLString:args[@"html"] baseURL:[NSURL URLWithString:args[@"baseURL"] ?: @"about:blank"]];
    } else if ([op isEqualToString:@"decide"]) {
        void (^decide)(WKNavigationActionPolicy) = owner.decisions[args[@"id"]];
        [owner.decisions removeObjectForKey:args[@"id"]];
        if (decide) decide([args[@"allow"] boolValue] ? WKNavigationActionPolicyAllow : WKNavigationActionPolicyCancel);
    } else if ([op isEqualToString:@"back"]) [view goBack];
    else if ([op isEqualToString:@"forward"]) [view goForward];
    else if ([op isEqualToString:@"reload"]) [view reload];
    else if ([op isEqualToString:@"stop"]) [view stopLoading];
    else if ([op isEqualToString:@"focus"]) [view.window makeFirstResponder:view];
    else if ([op isEqualToString:@"visible"]) {
        owner.visible = [args boolValue];
        view.hidden = !owner.visible || view.frame.size.width <= 0 || view.frame.size.height <= 0;
    }
    else if ([op isEqualToString:@"zoom"]) {
        double zoom = [args doubleValue];
        if (zoom < 0.25 || zoom > 5) return strdup("{\"error\":\"Zoom must be between 0.25 and 5\"}");
        view.pageZoom = zoom;
    } else if ([op isEqualToString:@"evaluate"]) {
        NSNumber *request = args[@"id"];
        NSDictionary *arguments = @{ @"source": args[@"script"] };
        void (^done)(id, NSError *) = ^(id value, NSError *error) {
            if (!owner.callback) return;
            NSMutableDictionary *event = [@{ @"type": @"evaluation", @"id": request } mutableCopy];
            if (error) event[@"error"] = error.userInfo[@"WKJavaScriptExceptionMessage"] ?: error.localizedDescription;
            else if (value) event[@"value"] = value;
            AppsEmit(owner, event);
        };
        // WARNING: page CSP blocks eval in the page world, so those pages run the script in an isolated world, which has its own globals.
        [view callAsyncJavaScript:@"return await (0,eval)(source)" arguments:arguments inFrame:nil inContentWorld:WKContentWorld.pageWorld completionHandler:^(id value, NSError *error) {
            if ([error.userInfo[@"WKJavaScriptExceptionMessage"] hasPrefix:@"EvalError: Refused to evaluate"]) [owner.view callAsyncJavaScript:@"return await (0,eval)(source)" arguments:arguments inFrame:nil inContentWorld:[WKContentWorld worldWithName:@"apps"] completionHandler:done];
            else done(value, error);
        }];
    } else if ([op hasPrefix:@"cookies."]) {
        NSNumber *request = args[@"id"];
        WKHTTPCookieStore *store = view.configuration.websiteDataStore.httpCookieStore;
        if ([op isEqualToString:@"cookies.set"]) {
            NSDictionary *details = args[@"details"];
            NSURL *url = [NSURL URLWithString:details[@"url"]];
            NSMutableDictionary *properties = [@{ NSHTTPCookieName: details[@"name"], NSHTTPCookieValue: details[@"value"], NSHTTPCookieDomain: details[@"domain"] ?: url.host ?: @"", NSHTTPCookiePath: details[@"path"] ?: @"/", NSHTTPCookieOriginURL: url ?: [NSURL URLWithString:@"about:blank"] } mutableCopy];
            if ([details[@"secure"] boolValue]) properties[NSHTTPCookieSecure] = @"TRUE";
            if ([details[@"httpOnly"] boolValue]) properties[@"HttpOnly"] = @"TRUE";
            if (details[@"expirationDate"]) properties[NSHTTPCookieExpires] = [NSDate dateWithTimeIntervalSince1970:[details[@"expirationDate"] doubleValue]];
            NSHTTPCookie *cookie = [NSHTTPCookie cookieWithProperties:properties];
            if (!cookie) return strdup("{\"error\":\"Invalid cookie properties\"}");
            [store setCookie:cookie completionHandler:^{ AppsEmit(owner, @{ @"type": @"evaluation", @"id": request, @"value": [NSNull null] }); }];
        } else {
            [store getAllCookies:^(NSArray<NSHTTPCookie *> *cookies) {
                NSMutableArray *matches = [NSMutableArray new];
                NSDictionary *filter = args[@"filter"] ?: @{};
                NSURL *url = filter[@"url"] ? [NSURL URLWithString:filter[@"url"]] : nil;
                dispatch_group_t group = dispatch_group_create();
                for (NSHTTPCookie *cookie in cookies) {
                    NSString *domain = [cookie.domain hasPrefix:@"."] ? [cookie.domain substringFromIndex:1] : cookie.domain;
                    if (filter[@"name"] && ![cookie.name isEqual:filter[@"name"]]) continue;
                    if (filter[@"domain"] && ![cookie.domain isEqual:filter[@"domain"]]) continue;
                    if (filter[@"path"] && ![cookie.path isEqual:filter[@"path"]]) continue;
                    if (url && ![url.host isEqual:domain] && !([cookie.domain hasPrefix:@"."] && [url.host hasSuffix:[@"." stringByAppendingString:domain]])) continue;
                    NSString *requestPath = url.path.length ? url.path : @"/";
                    if (url && (![requestPath hasPrefix:cookie.path] || (requestPath.length > cookie.path.length && ![cookie.path hasSuffix:@"/"] && [requestPath characterAtIndex:cookie.path.length] != '/') || (cookie.secure && ![url.scheme isEqual:@"https"]))) continue;
                    if ([op isEqualToString:@"cookies.remove"]) {
                        dispatch_group_enter(group);
                        [store deleteCookie:cookie completionHandler:^{ dispatch_group_leave(group); }];
                    } else [matches addObject:@{ @"name": cookie.name, @"value": cookie.value, @"domain": cookie.domain, @"path": cookie.path, @"secure": @(cookie.secure), @"httpOnly": @(cookie.HTTPOnly), @"session": @(!cookie.expiresDate), @"expirationDate": cookie.expiresDate ? @(cookie.expiresDate.timeIntervalSince1970) : [NSNull null] }];
                }
                dispatch_group_notify(group, dispatch_get_main_queue(), ^{ AppsEmit(owner, @{ @"type": @"evaluation", @"id": request, @"value": [op isEqualToString:@"cookies.remove"] ? [NSNull null] : matches }); });
            }];
        }
    } else if ([op isEqualToString:@"clearStorage"]) {
        NSNumber *request = args[@"id"];
        [view.configuration.websiteDataStore removeDataOfTypes:WKWebsiteDataStore.allWebsiteDataTypes modifiedSince:NSDate.distantPast completionHandler:^{ AppsEmit(owner, @{ @"type": @"evaluation", @"id": request, @"value": [NSNull null] }); }];
    } else if ([op isEqualToString:@"capture"]) {
        NSNumber *request = args[@"id"];
        WKSnapshotConfiguration *configuration = [WKSnapshotConfiguration new];
        if (args[@"rect"]) {
            NSDictionary *rect = args[@"rect"];
            configuration.rect = NSMakeRect([rect[@"x"] doubleValue], [rect[@"y"] doubleValue], [rect[@"width"] doubleValue], [rect[@"height"] doubleValue]);
        }
        [view takeSnapshotWithConfiguration:configuration completionHandler:^(NSImage *image, NSError *error) {
            if (!owner.callback) return;
            if (error) { AppsEmit(owner, @{ @"type": @"evaluation", @"id": request, @"error": error.localizedDescription }); return; }
            NSBitmapImageRep *bitmap = [NSBitmapImageRep imageRepWithData:image.TIFFRepresentation];
            NSData *data = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
            AppsEmit(owner, data ? @{ @"type": @"evaluation", @"id": request, @"value": @{ @"png": [data base64EncodedStringWithOptions:0], @"width": @(bitmap.pixelsWide), @"height": @(bitmap.pixelsHigh) } } : @{ @"type": @"evaluation", @"id": request, @"error": @"Could not encode browser snapshot" });
        }];
    } else if ([op isEqualToString:@"inspect"]) {
        if (@available(macOS 13.3, *)) view.inspectable = YES;
        // WARNING: WebKit has no public API for opening its inspector window.
        SEL inspectorSelector = NSSelectorFromString(@"_inspector");
        if (![view respondsToSelector:inspectorSelector]) return strdup("{\"error\":\"Use Safari Develop to inspect this view\"}");
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Warc-performSelector-leaks"
        id inspector = [view performSelector:inspectorSelector];
        SEL show = NSSelectorFromString(@"show");
        if (![inspector respondsToSelector:show]) return strdup("{\"error\":\"Use Safari Develop to inspect this view\"}");
        [inspector performSelector:show];
#pragma clang diagnostic pop
    } else if ([op isEqualToString:@"state"]) {
        result = @{ @"url": view.URL.absoluteString ?: @"", @"title": view.title ?: @"", @"loading": @(view.loading), @"canGoBack": @(view.canGoBack), @"canGoForward": @(view.canGoForward), @"zoomFactor": @(view.pageZoom), @"closed": @(owner.closed) };
    } else return strdup("{\"error\":\"Unknown browser operation\"}");
    return strdup(AppsJSON(@{ @"value": result }).UTF8String);
}

void apps_browser_free(char *text) { free(text); }
