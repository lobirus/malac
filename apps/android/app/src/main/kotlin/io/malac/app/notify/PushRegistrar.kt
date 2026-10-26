package io.malac.app.notify

import io.malac.app.data.SavedDaemon

/**
 * The seam for remote push. A daemon cannot reach a phone that is asleep,
 * and FCM needs server credentials that must not ship in an open-source
 * daemon, so push needs a relay: the daemon sends "request R of daemon D is
 * waiting" (ids only, no content) to a relay that holds the FCM credentials,
 * and the relay wakes the device, which fetches the details itself over its
 * own authenticated connection. An implementation would get an FCM token and
 * register it with the daemon for this device; the background check and the
 * notifications stay as they are.
 */
interface PushRegistrar {
    suspend fun register(daemon: SavedDaemon)

    suspend fun unregister(daemon: SavedDaemon)
}

/** No relay exists yet. */
object NoPushRegistrar : PushRegistrar {
    override suspend fun register(daemon: SavedDaemon) = Unit

    override suspend fun unregister(daemon: SavedDaemon) = Unit
}
