import { useState } from "react"
import {
  BellIcon,
  BoldIcon,
  CreditCardIcon,
  InfoIcon,
  ItalicIcon,
  LogOutIcon,
  MoonIcon,
  SettingsIcon,
  SunIcon,
  UnderlineIcon,
  UserIcon,
} from "lucide-react"

import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from "@/components/ui/accordion"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import {
  Avatar,
  AvatarFallback,
  AvatarGroup,
  AvatarGroupCount,
} from "@/components/ui/avatar"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Popover,
  PopoverContent,
  PopoverDescription,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
} from "@/components/ui/popover"
import { Progress } from "@/components/ui/progress"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Separator } from "@/components/ui/separator"
import { Skeleton } from "@/components/ui/skeleton"
import { Slider } from "@/components/ui/slider"
import { Switch } from "@/components/ui/switch"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Textarea } from "@/components/ui/textarea"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip"

const roles = [
  { label: "Developer", value: "developer" },
  { label: "Designer", value: "designer" },
  { label: "Manager", value: "manager" },
]

const invoices = [
  { id: "INV001", status: "Paid", method: "Credit Card", amount: "$250.00" },
  { id: "INV002", status: "Pending", method: "PayPal", amount: "$150.00" },
  { id: "INV003", status: "Unpaid", method: "Bank Transfer", amount: "$350.00" },
  { id: "INV004", status: "Paid", method: "Credit Card", amount: "$450.00" },
  { id: "INV005", status: "Paid", method: "PayPal", amount: "$550.00" },
]

export function App() {
  const [dark, setDark] = useState(false)
  const [volume, setVolume] = useState(40)

  const toggleTheme = () => {
    document.documentElement.classList.toggle("dark", !dark)
    setDark(!dark)
  }

  return (
    <main className="mx-auto flex max-w-4xl flex-col gap-6 p-6 md:p-10">
      <header className="flex items-center justify-between gap-4">
        <div className="flex flex-col gap-1">
          <h1 className="text-2xl font-semibold tracking-tight">shadcn/ui</h1>
          <p className="text-sm text-muted-foreground">
            Base UI components and Tailwind CSS in a native window.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Tooltip>
            <TooltipTrigger
              render={
                <Button variant="outline" size="icon" onClick={toggleTheme} />
              }
            >
              {dark ? <SunIcon /> : <MoonIcon />}
            </TooltipTrigger>
            <TooltipContent>Toggle theme</TooltipContent>
          </Tooltip>
          <DropdownMenu>
            <DropdownMenuTrigger render={<Button variant="outline" />}>
              <UserIcon data-icon="inline-start" />
              Account
            </DropdownMenuTrigger>
            <DropdownMenuContent className="w-48" align="end">
              <DropdownMenuGroup>
                <DropdownMenuLabel>My Account</DropdownMenuLabel>
                <DropdownMenuItem>
                  <UserIcon />
                  Profile
                  <DropdownMenuShortcut>⇧⌘P</DropdownMenuShortcut>
                </DropdownMenuItem>
                <DropdownMenuItem>
                  <CreditCardIcon />
                  Billing
                  <DropdownMenuShortcut>⌘B</DropdownMenuShortcut>
                </DropdownMenuItem>
                <DropdownMenuItem>
                  <SettingsIcon />
                  Settings
                  <DropdownMenuShortcut>⌘S</DropdownMenuShortcut>
                </DropdownMenuItem>
              </DropdownMenuGroup>
              <DropdownMenuSeparator />
              <DropdownMenuItem variant="destructive">
                <LogOutIcon />
                Log out
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </header>

      <Tabs defaultValue="account">
        <TabsList>
          <TabsTrigger value="account">Account</TabsTrigger>
          <TabsTrigger value="components">Components</TabsTrigger>
          <TabsTrigger value="invoices">Invoices</TabsTrigger>
        </TabsList>

        <TabsContent value="account" className="pt-2">
          <Card>
            <CardHeader>
              <CardTitle>Profile</CardTitle>
              <CardDescription>
                This is how others will see you.
              </CardDescription>
            </CardHeader>
            <CardContent className="grid gap-5 md:grid-cols-2">
              <div className="grid gap-2">
                <Label htmlFor="name">Name</Label>
                <Input id="name" placeholder="Ada Lovelace" />
              </div>
              <div className="grid gap-2">
                <Label htmlFor="email">Email</Label>
                <Input id="email" type="email" placeholder="ada@example.com" />
              </div>
              <div className="grid gap-2">
                <Label>Role</Label>
                <Select items={roles} defaultValue="developer">
                  <SelectTrigger className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {roles.map(role => (
                      <SelectItem key={role.value} value={role.value}>
                        {role.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="grid gap-2">
                <Label>Plan</Label>
                <RadioGroup defaultValue="pro" className="flex gap-4 pt-1.5">
                  {["Free", "Pro", "Team"].map(plan => (
                    <Label key={plan} className="font-normal">
                      <RadioGroupItem value={plan.toLowerCase()} />
                      {plan}
                    </Label>
                  ))}
                </RadioGroup>
              </div>
              <div className="grid gap-2 md:col-span-2">
                <Label htmlFor="bio">Bio</Label>
                <Textarea id="bio" placeholder="Tell us a little about yourself" />
              </div>
              <Label className="font-normal">
                <Checkbox defaultChecked />
                Accept terms and conditions
              </Label>
              <Label className="justify-between font-normal md:justify-end">
                Email notifications
                <Switch defaultChecked />
              </Label>
            </CardContent>
            <CardFooter className="justify-end gap-2">
              <Button variant="ghost">Cancel</Button>
              <Dialog>
                <DialogTrigger render={<Button />}>Save changes</DialogTrigger>
                <DialogContent>
                  <DialogHeader>
                    <DialogTitle>Save profile?</DialogTitle>
                    <DialogDescription>
                      Your profile will be visible to everyone on your team.
                    </DialogDescription>
                  </DialogHeader>
                  <DialogFooter>
                    <DialogClose render={<Button variant="outline" />}>
                      Cancel
                    </DialogClose>
                    <DialogClose render={<Button />}>Save</DialogClose>
                  </DialogFooter>
                </DialogContent>
              </Dialog>
            </CardFooter>
          </Card>
        </TabsContent>

        <TabsContent value="components" className="grid gap-4 pt-2 md:grid-cols-2">
          <Card>
            <CardHeader>
              <CardTitle>Buttons</CardTitle>
              <CardDescription>Variants and sizes.</CardDescription>
            </CardHeader>
            <CardContent className="flex flex-wrap gap-2">
              <Button>Default</Button>
              <Button variant="secondary">Secondary</Button>
              <Button variant="outline">Outline</Button>
              <Button variant="ghost">Ghost</Button>
              <Button variant="destructive">Destructive</Button>
              <Button variant="link">Link</Button>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Badges and toggles</CardTitle>
              <CardDescription>Small status labels.</CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-4">
              <div className="flex flex-wrap gap-2">
                <Badge>Default</Badge>
                <Badge variant="secondary">Secondary</Badge>
                <Badge variant="outline">Outline</Badge>
                <Badge variant="destructive">Destructive</Badge>
              </div>
              <ToggleGroup variant="outline" multiple defaultValue={["bold"]}>
                <ToggleGroupItem value="bold" aria-label="Bold">
                  <BoldIcon />
                </ToggleGroupItem>
                <ToggleGroupItem value="italic" aria-label="Italic">
                  <ItalicIcon />
                </ToggleGroupItem>
                <ToggleGroupItem value="underline" aria-label="Underline">
                  <UnderlineIcon />
                </ToggleGroupItem>
              </ToggleGroup>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Slider and progress</CardTitle>
              <CardDescription>{`Volume: ${volume}%`}</CardDescription>
            </CardHeader>
            <CardContent className="flex flex-col gap-5">
              <Slider
                value={volume}
                onValueChange={value => setVolume(value as number)}
                max={100}
              />
              <Progress value={volume} />
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Team</CardTitle>
              <CardDescription>Avatars, popover and skeleton.</CardDescription>
            </CardHeader>
            <CardContent className="flex items-center justify-between gap-4">
              <AvatarGroup>
                {["AL", "GH", "AT"].map(name => (
                  <Avatar key={name}>
                    <AvatarFallback>{name}</AvatarFallback>
                  </Avatar>
                ))}
                <AvatarGroupCount>+4</AvatarGroupCount>
              </AvatarGroup>
              <Popover>
                <PopoverTrigger render={<Button variant="outline" />}>
                  <BellIcon data-icon="inline-start" />
                  Invite
                </PopoverTrigger>
                <PopoverContent align="end">
                  <PopoverHeader>
                    <PopoverTitle>Invite a teammate</PopoverTitle>
                    <PopoverDescription>
                      They get an email with a link.
                    </PopoverDescription>
                  </PopoverHeader>
                  <Input placeholder="name@example.com" />
                </PopoverContent>
              </Popover>
            </CardContent>
            <CardFooter className="flex flex-col items-stretch gap-2 border-t">
              <Skeleton className="h-4 w-3/4" />
              <Skeleton className="h-4 w-1/2" />
            </CardFooter>
          </Card>

          <Alert className="md:col-span-2">
            <InfoIcon />
            <AlertTitle>Heads up!</AlertTitle>
            <AlertDescription>
              These are the real shadcn/ui components, copied in with the shadcn CLI.
            </AlertDescription>
          </Alert>

          <Accordion className="md:col-span-2" defaultValue={["item-1"]}>
            <AccordionItem value="item-1">
              <AccordionTrigger>Is it accessible?</AccordionTrigger>
              <AccordionContent>
                Yes. Base UI follows the WAI-ARIA design patterns.
              </AccordionContent>
            </AccordionItem>
            <AccordionItem value="item-2">
              <AccordionTrigger>Is it styled?</AccordionTrigger>
              <AccordionContent>
                Yes. The components use Tailwind CSS classes and CSS variables.
              </AccordionContent>
            </AccordionItem>
            <AccordionItem value="item-3">
              <AccordionTrigger>Is it animated?</AccordionTrigger>
              <AccordionContent>
                Yes. The animations come from tw-animate-css.
              </AccordionContent>
            </AccordionItem>
          </Accordion>
        </TabsContent>

        <TabsContent value="invoices" className="pt-2">
          <Card>
            <CardHeader>
              <CardTitle>Recent invoices</CardTitle>
              <CardDescription>Your last five invoices.</CardDescription>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Invoice</TableHead>
                    <TableHead>Status</TableHead>
                    <TableHead>Method</TableHead>
                    <TableHead className="text-right">Amount</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {invoices.map(invoice => (
                    <TableRow key={invoice.id}>
                      <TableCell className="font-medium">{invoice.id}</TableCell>
                      <TableCell>
                        <Badge variant={invoice.status === "Unpaid" ? "destructive" : "secondary"}>
                          {invoice.status}
                        </Badge>
                      </TableCell>
                      <TableCell>{invoice.method}</TableCell>
                      <TableCell className="text-right">{invoice.amount}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>

      <Separator />
      <p className="text-sm text-muted-foreground">
        Use the button at the top right to switch between light and dark.
      </p>
    </main>
  )
}
