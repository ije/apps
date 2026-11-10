import { flushSync } from "react-dom"
import { createRoot } from "react-dom/client"

import { App } from "@/App"
import { TooltipProvider } from "@/components/ui/tooltip"

const root = createRoot(document.getElementById("root")!)
flushSync(() =>
  root.render(
    <TooltipProvider>
      <App />
    </TooltipProvider>
  )
)
