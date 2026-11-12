#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>
#include <stdio.h>
#include <stdlib.h>
void *apps_browser_create(void *,const char *,const char *,void (*)(void *,const char *),void *);
void apps_browser_destroy(void *);
void apps_browser_attach(void *, void *);
void apps_browser_bounds(void *, double, double, double, double);
char *apps_browser_command(void *,const char *,const char *);
void apps_browser_free(char *);
static void *browser;
static int done;
static void event(void *context,const char *message) {
    NSDictionary *value=[NSJSONSerialization JSONObjectWithData:[[NSString stringWithUTF8String:message] dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
    if ([value[@"type"] isEqual:@"did-finish-load"]) {
        apps_browser_free(apps_browser_command(browser,"evaluate","{\"id\":1,\"script\":\"Promise.resolve(document.getElementById('title').textContent + ':' + (6 * 7) + ':' + typeof process)\"}"));
    } else if ([value[@"type"] isEqual:@"evaluation"] && [value[@"id"] intValue]==1) {
        if (![value[@"value"] isEqual:@"WebKit:42:undefined"]) { done=2; return; }
        printf("%s\n",message);
        apps_browser_free(apps_browser_command(browser,"cookies.set","{\"id\":2,\"details\":{\"url\":\"https://example.test/\",\"name\":\"apps-test\",\"value\":\"hello\",\"secure\":true}}"));
    } else if ([value[@"type"] isEqual:@"evaluation"] && [value[@"id"] intValue]==2) {
        if (value[@"error"]) { done=2; return; }
        apps_browser_free(apps_browser_command(browser,"cookies.get","{\"id\":3,\"filter\":{\"url\":\"https://example.test/\"}}"));
    } else if ([value[@"type"] isEqual:@"evaluation"] && [value[@"id"] intValue]==3) {
        NSArray *cookies=value[@"value"];
        if (![cookies isKindOfClass:[NSArray class]] || cookies.count!=1 || ![cookies[0][@"value"] isEqual:@"hello"]) { done=2; return; }
        apps_browser_free(apps_browser_command(browser,"cookies.remove","{\"id\":4,\"filter\":{\"url\":\"https://example.test/\",\"name\":\"apps-test\"}}"));
    } else if ([value[@"type"] isEqual:@"evaluation"] && [value[@"id"] intValue]==4) {
        apps_browser_free(apps_browser_command(browser,"cookies.get","{\"id\":5,\"filter\":{}}"));
    } else if ([value[@"type"] isEqual:@"evaluation"] && [value[@"id"] intValue]==5) {
        if (![value[@"value"] isKindOfClass:[NSArray class]] || [value[@"value"] count]!=0) { done=2; return; }
        apps_browser_free(apps_browser_command(browser,"evaluate","{\"id\":6,\"script\":\"'x'.repeat(4 * 1024 * 1024)\"}"));
    } else if ([value[@"type"] isEqual:@"evaluation"] && [value[@"id"] intValue]==6) {
        done=[value[@"error"] isEqual:@"Browser result exceeds 4 MiB"]?1:2;
    } else if ([value[@"type"] isEqual:@"did-fail-load"]) done=2;
}
int main(int argc, const char *argv[]) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
        __weak id owner;
        @autoreleasepool {
        browser=apps_browser_create(NULL,"{\"show\":false,\"inspectable\":true}",NULL,event,NULL);
        if (!browser) return 3;
        owner=(__bridge id)browser;
        NSWindow *parent=[[NSWindow alloc] initWithContentRect:NSMakeRect(0,0,1000,800) styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
        parent.releasedWhenClosed=NO;
        apps_browser_bounds(browser,10,20,300,200);
        apps_browser_attach(browser,(__bridge void *)parent.contentView);
        NSRect frame=parent.contentView.subviews.lastObject.frame;
        if (frame.origin.x!=10 || frame.origin.y!=580 || frame.size.width!=300 || frame.size.height!=200) return 6;
        NSView *native=parent.contentView.subviews.lastObject;
        apps_browser_free(apps_browser_command(browser,"visible","false"));
        apps_browser_bounds(browser,10,20,300,200);
        if (!native.hidden) return 7;
        apps_browser_free(apps_browser_command(browser,"visible","true"));
        if (native.hidden) return 8;
        apps_browser_attach(browser,NULL);
        if (native.superview) return 9;
        apps_browser_attach(browser,(__bridge void *)parent.contentView);
        if (native.superview!=parent.contentView || native.frame.origin.y!=580) return 10;
        [parent.contentView addSubview:[[NSView alloc] initWithFrame:NSZeroRect]];
        apps_browser_attach(browser,(__bridge void *)parent.contentView);
        if (parent.contentView.subviews.lastObject!=native) return 11;
        apps_browser_free(apps_browser_command(browser,"loadHTML","{\"html\":\"<h1 id='title'>WebKit</h1>\",\"baseURL\":\"about:blank\"}"));
        NSDate *deadline=[NSDate dateWithTimeIntervalSinceNow:15];
        while (!done && [deadline timeIntervalSinceNow]>0) @autoreleasepool { [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.02]]; }
        apps_browser_free(apps_browser_command(browser,"evaluate","{\"id\":7,\"script\":\"new Promise(r=>window.keep=r)\"}"));
        apps_browser_destroy(browser);
        [parent close];
        }
        for (int i=0; i<10 && owner; i++) @autoreleasepool { [[NSRunLoop currentRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.02]]; }
        if (done!=1) return 4;
        if (owner) return 12;
        return 0;
    }
}
