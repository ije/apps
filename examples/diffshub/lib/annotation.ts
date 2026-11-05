export const annotationCardBase =
  'm-2 flex max-w-[600px] gap-2.5 rounded-xl border border-[var(--diffshub-annotation-border,var(--color-border))] bg-[var(--diffshub-annotation-bg,var(--color-card))] bg-clip-padding p-3 font-sans text-[var(--diffshub-annotation-fg,var(--color-card-foreground))] shadow-[var(--diffshub-annotation-shadow,0_2px_4px_rgb(0_0_0_/_0.025),0_4px_8px_rgb(0_0_0_/_0.025))]';

// All available reviewer personas, bundled from ../diffshub-avatars/.
const AVATAR_URLS = {
  alex: new URL('../diffshub-avatars/alex.png', import.meta.url).href,
  amacateus: new URL('../diffshub-avatars/amacateus.png', import.meta.url).href,
  amadeus: new URL('../diffshub-avatars/amadeus.png', import.meta.url).href,
  aussie: new URL('../diffshub-avatars/aussie.png', import.meta.url).href,
  cedric: new URL('../diffshub-avatars/cedric.png', import.meta.url).href,
  chugs: new URL('../diffshub-avatars/chugs.png', import.meta.url).href,
  dwayn: new URL('../diffshub-avatars/dwayn.png', import.meta.url).href,
  ed: new URL('../diffshub-avatars/ed.png', import.meta.url).href,
  fat: new URL('../diffshub-avatars/fat.png', import.meta.url).href,
  ian: new URL('../diffshub-avatars/ian.png', import.meta.url).href,
  jacob2: new URL('../diffshub-avatars/jacob2.png', import.meta.url).href,
  joe: new URL('../diffshub-avatars/joe.png', import.meta.url).href,
  kris: new URL('../diffshub-avatars/kris.png', import.meta.url).href,
  mdo: new URL('../diffshub-avatars/mdo.png', import.meta.url).href,
  murphy: new URL('../diffshub-avatars/murphy.png', import.meta.url).href,
  nicolas: new URL('../diffshub-avatars/nicolas.png', import.meta.url).href,
  pia: new URL('../diffshub-avatars/pia.png', import.meta.url).href,
  toshi: new URL('../diffshub-avatars/toshi.png', import.meta.url).href,
  zac: new URL('../diffshub-avatars/zac.png', import.meta.url).href,
} as const;

const AVATAR_NAMES = Object.keys(AVATAR_URLS) as (keyof typeof AVATAR_URLS)[];

export type AvatarName = (typeof AVATAR_NAMES)[number];

export interface Persona {
  name: AvatarName;
  avatarSrc: string;
}

// Triggers browser fetches for all avatar images so they are in the cache
// before the comment form opens. Call once on mount of the top-level UI component.
export function preloadAvatars(): void {
  for (const name of AVATAR_NAMES) {
    const img = new Image();
    img.src = AVATAR_URLS[name];
  }
}

function buildPersona(name: AvatarName): Persona {
  return { name, avatarSrc: AVATAR_URLS[name] };
}

// Picks a random persona from the avatar list. Intended for use as a useState
// lazy initializer so each new draft form gets a fresh identity on mount.
export function getRandomPersona(): Persona {
  const name = AVATAR_NAMES[Math.floor(Math.random() * AVATAR_NAMES.length)];
  return buildPersona(name);
}

// Returns a persona for the given name or seed. If the seed is an exact avatar
// name (i.e. it was stored directly from getRandomPersona), returns that persona
// directly so draft and saved annotations stay in sync. Otherwise falls back to
// a djb2 hash to spread arbitrary comment keys across the avatar list.
export function getCommentPersona(seed: string): Persona {
  if (AVATAR_NAMES.includes(seed as AvatarName)) {
    return buildPersona(seed as AvatarName);
  }
  let hash = 5381;
  for (let i = 0; i < seed.length; i++) {
    hash = ((hash << 5) + hash + seed.charCodeAt(i)) >>> 0;
  }
  return buildPersona(AVATAR_NAMES[hash % AVATAR_NAMES.length]);
}
