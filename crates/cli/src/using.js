const babelHelpers={usingCtx(){
const none={},stack=[];
const use=(value,async)=>{
if(value==null){if(async)stack.push({async});return value}
if(Object(value)!==value)throw new TypeError('A using declaration needs an object, null or undefined');
let dispose=async?value[Symbol.asyncDispose]:void 0;
const fallback=dispose==null;
if(fallback)dispose=value[Symbol.dispose];
if(typeof dispose!='function')throw new TypeError('The value of a using declaration is not disposable');
stack.push({value,dispose,async,fallback});
return value};
const context={e:none,u:value=>use(value,false),a:value=>use(value,true),d(){
let error=context.e,needsAwait=false,hasAwaited=false;
const steps=function*(){
while(stack.length){
const{value,dispose,async,fallback}=stack.pop();
if(!async&&needsAwait&&!hasAwaited){yield;needsAwait=false}
if(!dispose){needsAwait=true;continue}
try{const result=dispose.call(value);if(async){hasAwaited=true;yield fallback?void 0:result}}
catch(thrown){error=error===none?thrown:new SuppressedError(thrown,error)}}
if(needsAwait&&!hasAwaited)yield;
if(error!==none)throw error}();
const step=(method,value)=>{const result=steps[method](value);if(!result.done)return Promise.resolve(result.value).then(value=>step('next',value),thrown=>step('throw',thrown))};
return step('next')}};
return context}};
