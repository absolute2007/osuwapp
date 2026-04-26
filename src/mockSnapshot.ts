import type { AppSnapshot } from './types'

export const initialSnapshot: AppSnapshot = {
  connection: {
    status: 'searching',
    detail: 'Looking for osu!.exe.',
    updatedAtMs: Date.now(),
  },
  session: null,
  recentPlays: [],
}
