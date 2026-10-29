package io.malac.app.notify

import io.malac.app.data.SavedDaemon

/**
 * Remote push, so approvals reach the phone while the app is closed. The
 * daemon pushes to an endpoint this device registered with it, encrypted
 * for this device (see `docs/remote.md`); [UnifiedPushRegistrar] gets the
 * endpoint from a UnifiedPush distributor. Without one, the background
 * check still runs every 15 minutes.
 */
interface PushRegistrar {
    suspend fun register(daemon: SavedDaemon)

    suspend fun unregister(daemon: SavedDaemon)
}
