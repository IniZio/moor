// Package githome fetches a repository from a forge over smart HTTP into memory. The portal reads the team's
// home from it, so it needs no folder, volume or git binary. It touches no disk, so a read-only root filesystem
// is enough.
package githome
