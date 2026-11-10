#include "config.h"

#if ENABLE(AOT)
#include "AOTBuiltinTableBindings.h"
#include "AOTCompileOptionWords.h"
#include "AOTDriver.h"
#include "AOTHost.h"
#include "AOTImageLoading.h"
#include "AOTImageFormat.h"
#include "AOTModuleHost.h"
#include "AOTSchemaEnums.h"
#include "APICast.h"
#include "CallFrameInlines.h"
#include "Error.h"
#include "GetterSetter.h"
#include "JSArray.h"
#include "JSCInlines.h"
#include "JSNativeStdFunction.h"
#include "APIUtils.h"
#include "InitializeThreading.h"
#include "JSContextRef.h"
#include "JSGlobalObject.h"
#include "JSLock.h"
#include "JSObjectRef.h"
#include "JSObjectInlines.h"
#include "Options.h"
#include "ObjectConstructor.h"
#include <cstdio>
#include <cstring>
#include <mutex>
#include <wtf/text/MakeString.h>

using namespace JSC;

static void initializeAOT()
{
    initialize();
    static std::once_flag helpers;
    std::call_once(helpers, [] { AOT::BuiltinBindings::bindNativeHelpersOfEngine(); });
}

static JSValue requireSpecifier(JSGlobalObject* global, JSObject*, JSValue specifier)
{
    auto scope = DECLARE_THROW_SCOPE(global->vm());
    String name = specifier.toWTFString(global);
    RETURN_IF_EXCEPTION(scope, { });
    if (auto module = AOT::findModule(name))
        RELEASE_AND_RETURN(scope, AOT::requireModule(global, *module));
    throwTypeError(global, scope, makeString("Cannot load AOT module: "_s, name));
    return { };
}

static JSFunction* createRequire(JSGlobalObject* global)
{
    return JSNativeStdFunction::create(global->vm(), global, 1, "require"_s, [](JSGlobalObject* realm, CallFrame* frame) {
        return JSValue::encode(requireSpecifier(realm, nullptr, frame->argument(0)));
    });
}

namespace JSC {
extern constinit const AOT::ModuleHost appsAOTModuleHost {
    .createModuleObject = [](JSGlobalObject* global, JSString* path, AOT::ModuleID number, OptionSet<AOT::ModuleObjectTrait> traits, AOT::ModuleObjects& objects) {
        VM& vm = global->vm();
        auto scope = DECLARE_THROW_SCOPE(vm);
        JSObject* module = constructEmptyObject(global);
        if (traits.contains(AOT::ModuleObjectTrait::HasExportsVariable)) {
            auto* get = JSNativeStdFunction::create(vm, global, 0, "get exports"_s, [number](JSGlobalObject* realm, CallFrame*) {
                return JSValue::encode(AOT::moduleExports(realm, number));
            });
            auto* set = JSNativeStdFunction::create(vm, global, 1, "set exports"_s, [number](JSGlobalObject* realm, CallFrame* frame) {
                AOT::setModuleExports(realm, number, frame->argument(0));
                return JSValue::encode(jsUndefined());
            });
            module->putDirectAccessor(global, vm.propertyNames->exports, GetterSetter::create(vm, global, get, set), static_cast<unsigned>(PropertyAttribute::Accessor));
        }
        module->putDirect(vm, vm.propertyNames->id, traits.contains(AOT::ModuleObjectTrait::IsMain) ? jsString(vm, String("."_s)) : path);
        module->putDirect(vm, Identifier::fromString(vm, "filename"_s), path);
        module->putDirect(vm, Identifier::fromString(vm, "loaded"_s), jsBoolean(true));
        JSArray* children = constructEmptyArray(global, nullptr);
        RETURN_IF_EXCEPTION(scope, void());
        auto* getChildren = JSNativeStdFunction::create(vm, global, 0, "get children"_s, [number, children](JSGlobalObject* realm, CallFrame*) -> EncodedJSValue {
            auto scope = DECLARE_THROW_SCOPE(realm->vm());
            bool shown = AOT::forEachNewChild(realm, number, [&](JSObject* child) { children->push(realm, child); });
            RETURN_IF_EXCEPTION(scope, { });
            return JSValue::encode(shown ? JSValue(children) : jsUndefined());
        }, children);
        module->putDirectAccessor(global, Identifier::fromString(vm, "children"_s), GetterSetter::create(vm, global, getChildren, nullptr), static_cast<unsigned>(PropertyAttribute::Accessor));
        auto* getParent = JSNativeStdFunction::create(vm, global, 0, "get parent"_s, [number](JSGlobalObject* realm, CallFrame*) {
            JSObject* parent = AOT::parentModuleObject(realm, number);
            return JSValue::encode(parent ? JSValue(parent) : jsNull());
        });
        module->putDirectAccessor(global, Identifier::fromString(vm, "parent"_s), GetterSetter::create(vm, global, getParent, nullptr), static_cast<unsigned>(PropertyAttribute::Accessor));
        auto* paths = constructEmptyArray(global, nullptr);
        RETURN_IF_EXCEPTION(scope, void());
        module->putDirect(vm, Identifier::fromString(vm, "paths"_s), paths);
        objects.module = module;
        objects.require = createRequire(global);
    },
    .createRequirer = [](JSGlobalObject* global, JSObject* importMeta, AOT::ModuleID, AOT::ModuleObjects& objects) {
        objects.module = importMeta;
        objects.require = createRequire(global);
    },
    .requireSpecifier = requireSpecifier,
    .resolveSpecifier = [](JSGlobalObject* global, JSObject*, JSValue specifier, AOT::ResolvedSpecifierForm) -> JSValue {
        auto scope = DECLARE_THROW_SCOPE(global->vm());
        String name = specifier.toWTFString(global);
        RETURN_IF_EXCEPTION(scope, { });
        if (auto module = AOT::findModule(name))
            return jsString(global->vm(), AOT::modulePath(*module));
        throwTypeError(global, scope, makeString("Cannot resolve AOT module: "_s, name));
        return { };
    },
    .captureSyntheticExports = [](JSGlobalObject* global, AOT::SyntheticExportRule, JSValue, WTF::Vector<Identifier, 4>&, MarkedArgumentBuffer&) -> JSObject* {
        auto scope = DECLARE_THROW_SCOPE(global->vm());
        throwTypeError(global, scope, "Dynamic AOT module namespaces are unsupported"_s);
        return nullptr;
    },
    .requireNativeModule = [](JSGlobalObject* global, JSString*) -> JSValue {
        auto scope = DECLARE_THROW_SCOPE(global->vm());
        throwTypeError(global, scope, "Native modules must use the apps module loader"_s);
        return { };
    },
    .captureNativeModuleExports = [](JSGlobalObject* global, uint32_t, WTF::Vector<Identifier, 4>&, MarkedArgumentBuffer&) -> JSObject* {
        auto scope = DECLARE_THROW_SCOPE(global->vm());
        throwTypeError(global, scope, "Native modules must use the apps module loader"_s);
        return nullptr;
    },
    .createAsyncModuleRequiredError = [](JSGlobalObject* global, const String& path) -> JSObject* {
        return createTypeError(global, makeString("Cannot require asynchronous AOT module: "_s, path));
    },
    .createLoadError = [](JSGlobalObject* global, const String& path) -> JSValue {
        return createTypeError(global, makeString("Cannot load AOT module: "_s, path));
    },
    .findModuleWithAttributes = [](JSGlobalObject*, const String&, const ScriptFetchParameters&) -> std::optional<AOT::ModuleID> { return std::nullopt; },
    .createModuleNotFoundError = [](JSGlobalObject* global, const String& specifier, const String&, bool) -> JSValue {
        return createTypeError(global, makeString("Cannot find AOT module: "_s, specifier));
    },
};
}

#if !APPS_JSC_AOT_RUNTIME
// Match build-jsc.sh's bun_revision: Bun's IR and built-in table must match this compiler.
extern "C" JS_EXPORT const char* JSGetAOTCompilerRevision()
{
    return "2596ac5d391d47804c93a42bdbe615804aa62387";
}

extern "C" JS_EXPORT bool JSCompileAOT(const uint8_t* bytes, size_t size, const char* path, uint32_t* entries, size_t count, size_t* layout, char* error, size_t capacity)
{
    initializeAOT();
    auto options = AOT::makeDefaultCompileOptions();
    options.admittedCode = AOT::Wire::AdmittedCode::ReplacedBuiltins;
    options.output = AOT::Wire::OutputOptions::RunsInSeveralRealms;
    // Use wide references to disable the cell cage, which keeps heap blocks resident (110 MB on DiffsHub's
    // stress test).
    options.target.referenceWidth = AOT::Wire::ReferenceWidth::Wide;
    auto output = AOT::compileProgram({ bytes, size }, options);
    auto image = output->section(AOT::Wire::ResultSection::Image);
    if (image.empty()) {
        auto text = output->section(AOT::Wire::ResultSection::Text);
        snprintf(error, capacity, "AOT compilation failed: %.*s", static_cast<int>(text.size()), text.data());
        return false;
    }
    auto ids = output->records<uint32_t>(AOT::Wire::ResultSection::EntryModuleIds);
    if (ids.size() != count) {
        snprintf(error, capacity, "AOT returned %zu entries; expected %zu", ids.size(), count);
        return false;
    }
    FILE* file = fopen(path, "wb");
    if (!file) {
        snprintf(error, capacity, "Cannot create AOT image");
        return false;
    }
    bool written = fwrite(image.data(), 1, image.size(), file) == image.size();
    written = !fclose(file) && written;
    if (!written) {
        snprintf(error, capacity, "Cannot write AOT image");
        return false;
    }
    const auto& header = *reinterpret_cast<const AOT::ImageHeader*>(image.data());
    layout[0] = header.codeOffset;
    layout[1] = header.executableSize;
    memcpy(entries, ids.data(), ids.size_bytes());
    return true;
}
#endif

extern "C" JS_EXPORT bool JSLoadAOT(const uint8_t* bytes, size_t size, char* error, size_t capacity)
{
    initializeAOT();
    auto verdict = AOT::useImage({ bytes, size }, std::nullopt, std::array<uint8_t, 16> { });
    if (verdict != AOT::Wire::ImageVerdict::Accepted) {
        snprintf(error, capacity, "Cannot load AOT image: %u", static_cast<unsigned>(verdict));
        return false;
    }
    return true;
}

extern "C" JS_EXPORT void JSAttachAOT(JSContextRef ctx)
{
    auto* global = toJS(ctx);
    JSLockHolder lock(global->vm());
    AOT::hostDidCreateRealm(global);
}

extern "C" JS_EXPORT JSObjectRef JSGetAOTFunction(JSContextRef ctx, uint32_t entry, JSValueRef* exception)
{
    auto* global = toJS(ctx);
    VM& vm = global->vm();
    JSLockHolder lock(vm);
    auto scope = DECLARE_TOP_EXCEPTION_SCOPE(vm);
    AOT::ModuleID module(entry - 1);
    JSValue function = AOT::requireModule(global, module);
    if (handleExceptionIfNeeded(scope, ctx, exception) == ExceptionStatus::DidThrow)
        return nullptr;
    return function.isObject() ? toRef(asObject(function)) : nullptr;
}
#endif
