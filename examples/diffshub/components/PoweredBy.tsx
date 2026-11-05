import { IconInfoFill } from '@pierre/icons';

import { StatusRow } from './StatusRow';

// Sidebar credits row linking the libraries diffshub is built on.
export function PoweredBy() {
  return (
    <div className="border-border shrink-0 border-b text-sm md:border-b-0">
      <StatusRow icon={IconInfoFill}>
        <div className="text-muted-foreground/75">
          Powered by{' '}
          <a
            href="https://diffs.com"
            target="_blank"
            rel="noopener noreferrer"
            className="inline-link text-muted-foreground hover:text-foreground no-underline"
          >
            Diffs
          </a>
          {', '}
          <a
            href="https://trees.software"
            target="_blank"
            rel="noopener noreferrer"
            className="inline-link text-muted-foreground hover:text-foreground no-underline"
          >
            Trees
          </a>
          {', and '}
          <a
            href="https://diffs.com/highlights"
            target="_blank"
            rel="noopener noreferrer"
            className="inline-link text-muted-foreground hover:text-foreground no-underline"
          >
            Highlights
          </a>
        </div>
      </StatusRow>
    </div>
  );
}
