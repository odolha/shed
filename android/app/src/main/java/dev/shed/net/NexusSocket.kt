package dev.shed.net

import android.util.Log
import dev.shed.api.API_VERSION
import dev.shed.api.ApiError
import dev.shed.api.Call
import dev.shed.api.ClientFrame
import dev.shed.api.ClientHello
import dev.shed.api.ErrorCode
import dev.shed.api.Event
import dev.shed.api.NotificationFrame
import dev.shed.api.Reply
import dev.shed.api.Request
import dev.shed.api.Response
import dev.shed.api.ServerFrame
import dev.shed.api.Subscribe
import dev.shed.api.Topic
import dev.shed.api.Unsubscribe
import dev.shed.api.WorkspaceInfo
import dev.shed.api.WorkspaceState
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.SendChannel
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout
import okhttp3.OkHttpClient
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import kotlin.random.Random

/** Where the connection to the nexus is. */
sealed interface Connection {
    /** Not started, or stopped (the app is in the background). */
    data object Idle : Connection

    data class Connecting(val attempt: Int) : Connection

    /** Welcomed: calls and subscriptions go through. */
    data object Online : Connection

    /** Lost; tries again at [retryAt] (milliseconds, wall clock). */
    data class Offline(val reason: String, val retryAt: Long) : Connection

    /** The nexus said goodbye (signed out, revoked, an API version it does not take). */
    data class Closed(val error: ApiError) : Connection
}

/** What a subscription gives: its events, and its end. */
sealed interface SubEvent {
    data class Data(val event: Event) : SubEvent

    /**
     * It ended without being asked to (the workspace went offline, the tab
     * closed). It starts again by itself when the workspace comes back.
     */
    data class Ended(val error: ApiError?) : SubEvent
}

/**
 * The one WebSocket to the nexus's `/api`: `Hello` → `Welcome`, requests
 * answered by id, subscriptions as flows. It reconnects with a backoff and
 * subscribes again to everything still watched; `Goodbye` ends it. It tells
 * the nexus whether the app is in view (`Visible`, after each welcome and on
 * change); while some app is, notifications come live ([notifications])
 * instead of being pushed.
 */
class NexusSocket(
    private val client: OkHttpClient,
    base: String,
    private val token: String,
    private val scope: CoroutineScope,
    private val clientName: String = "android",
    private val requestTimeoutMs: Long = 30_000,
    private val backoffMs: (attempt: Int) -> Long = ::defaultBackoff,
) {
    private val url = base.trimEnd('/') + "/api"

    private val _connection = MutableStateFlow<Connection>(Connection.Idle)
    val connection: StateFlow<Connection> = _connection.asStateFlow()

    private val _workspaces = MutableStateFlow<List<WorkspaceInfo>>(emptyList())
    val workspaces: StateFlow<List<WorkspaceInfo>> = _workspaces.asStateFlow()

    private val _states = MutableStateFlow<Map<String, WorkspaceState>>(emptyMap())
    val states: StateFlow<Map<String, WorkspaceState>> = _states.asStateFlow()

    private val _notifications = MutableSharedFlow<NotificationFrame>(extraBufferCapacity = 16)

    /** Notifications the nexus sends live, while some Shed app is in view. */
    val notifications: SharedFlow<NotificationFrame> = _notifications.asSharedFlow()

    /** The nexus's version, once welcomed. */
    var nexusVersion: String = ""
        private set

    private class Sub(val workspace: String, val topic: Topic, val out: SendChannel<SubEvent>) {
        var ended = false
    }

    private val lock = Any()
    private var socket: WebSocket? = null
    private var welcomed = false
    private var running = false
    private var visible = false
    private var attempt = 0
    private var nextId = 1u
    private var retryJob: Job? = null
    private val pending = HashMap<UInt, CompletableDeferred<Response>>()
    private val subs = HashMap<UInt, Sub>()

    /** Connects, unless connected or connecting already. */
    fun start() {
        synchronized(lock) {
            if (_connection.value is Connection.Closed) return
            running = true
            if (socket != null) return
            retryJob?.cancel()
            retryJob = null
            open()
        }
    }

    /** Connects now if waiting to try again. */
    fun retryNow() {
        synchronized(lock) {
            if (!running || socket != null) return
            retryJob?.cancel()
            retryJob = null
            open()
        }
    }

    /** Disconnects; subscriptions stay, to be sent again on `start`. */
    fun stop() {
        val ws = synchronized(lock) {
            running = false
            retryJob?.cancel()
            retryJob = null
            val ws = socket
            socket = null
            welcomed = false
            if (_connection.value !is Connection.Closed) _connection.value = Connection.Idle
            failPending(ApiError(ErrorCode.Offline, "disconnected"))
            ws
        }
        ws?.close(1000, "bye")
    }

    /** Tells the nexus whether the app is in view (now, and after every reconnect). */
    fun setVisible(inView: Boolean) {
        synchronized(lock) {
            if (visible == inView) return
            visible = inView
            if (welcomed) socket?.send(encode(ClientFrame.Visible(inView)))
        }
    }

    /** Sends a call to a workspace and waits for its answer. */
    suspend fun call(workspace: String, call: Call, timeoutMs: Long = requestTimeoutMs): Reply {
        val answer = CompletableDeferred<Response>()
        val id: UInt
        try {
            withTimeout(timeoutMs) {
                connection.first { it is Connection.Online || it is Connection.Closed }
            }
        } catch (_: TimeoutCancellationException) {
            throw ApiException.of(ErrorCode.Offline, "not connected to the nexus")
        }
        synchronized(lock) {
            val closed = _connection.value
            if (closed is Connection.Closed) throw ApiException(closed.error)
            id = nextId++
            pending[id] = answer
            val frame = ClientFrame.Request(Request(id, workspace, call))
            if (socket?.send(encode(frame)) != true) {
                pending.remove(id)
                throw ApiException.of(ErrorCode.Offline, "not connected to the nexus")
            }
        }
        val response = try {
            withTimeout(timeoutMs) { answer.await() }
        } catch (_: TimeoutCancellationException) {
            throw ApiException.of(ErrorCode.Timeout, "no answer in time")
        } finally {
            synchronized(lock) { pending.remove(id) }
        }
        response.error?.let { throw ApiException(it) }
        return response.ok ?: throw ApiException.of(ErrorCode.Failed, "an empty answer")
    }

    /**
     * Watches a topic while collected: the first event is the whole of it,
     * later ones its changes. Survives reconnects (subscribing again, which
     * starts with the whole of it once more).
     */
    fun subscribe(workspace: String, topic: Topic): Flow<SubEvent> = callbackFlow {
        val id = synchronized(lock) {
            val id = nextId++
            subs[id] = Sub(workspace, topic, channel)
            if (welcomed) socket?.send(encode(ClientFrame.Subscribe(Subscribe(id, workspace, topic))))
            id
        }
        awaitClose {
            synchronized(lock) {
                subs.remove(id)
                if (welcomed) socket?.send(encode(ClientFrame.Unsubscribe(Unsubscribe(id))))
            }
        }
    }.buffer(Channel.UNLIMITED)

    // ------------------------------------------------------------------ inside

    /** Under the lock. */
    private fun open() {
        attempt++
        _connection.value = Connection.Connecting(attempt)
        val request = okhttp3.Request.Builder()
            .url(url)
            .header("Authorization", "Bearer $token")
            .build()
        socket = client.newWebSocket(request, Listener())
    }

    private inner class Listener : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: okhttp3.Response) {
            webSocket.send(encode(ClientFrame.Hello(ClientHello(API_VERSION, clientName))))
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            val frame = try {
                ShedJson.decodeFromString(ServerFrame.serializer(), text)
            } catch (e: IllegalArgumentException) {
                Log.d(TAG, "a frame that does not read: ${e.message}")
                return
            }
            synchronized(lock) {
                if (socket !== webSocket) return
                handle(webSocket, frame)
            }
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
            lost(webSocket, reason.ifEmpty { "closed" })
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            lost(webSocket, reason.ifEmpty { "closed" })
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: okhttp3.Response?) {
            if (response?.code == 401) {
                closed(webSocket, ApiError(ErrorCode.Unauthorized, "not signed in"))
            } else {
                lost(webSocket, t.message ?: t.javaClass.simpleName)
            }
        }
    }

    /** Under the lock. */
    private fun handle(webSocket: WebSocket, frame: ServerFrame) {
        when (frame) {
            is ServerFrame.Welcome -> {
                val welcome = frame.content
                nexusVersion = welcome.nexus
                welcomed = true
                attempt = 0
                _workspaces.value = welcome.workspaces
                _states.value = welcome.states.associate { it.workspace to it.state }
                val online = welcome.workspaces.filter { it.online }.map { it.id }.toSet()
                for ((id, sub) in subs) {
                    if (sub.workspace in online) {
                        sub.ended = false
                        webSocket.send(encode(ClientFrame.Subscribe(Subscribe(id, sub.workspace, sub.topic))))
                    } else if (!sub.ended) {
                        sub.ended = true
                        sub.out.trySend(SubEvent.Ended(ApiError(ErrorCode.Offline, "the workspace is offline")))
                    }
                }
                webSocket.send(encode(ClientFrame.Visible(visible)))
                _connection.value = Connection.Online
            }
            is ServerFrame.Response -> pending.remove(frame.content.id)?.complete(frame.content)
            // An event a newer workspace sends that this version does not know: nothing to show.
            is ServerFrame.Event -> if (frame.content.event != Event.Unknown) {
                subs[frame.content.sub]?.out?.trySend(SubEvent.Data(frame.content.event))
            }
            is ServerFrame.Ended -> subs[frame.content.sub]?.let {
                it.ended = true
                it.out.trySend(SubEvent.Ended(frame.content.error))
            }
            is ServerFrame.Workspaces -> {
                val before = _workspaces.value.filter { it.online }.map { it.id }.toSet()
                _workspaces.value = frame.content
                val cameBack = frame.content.filter { it.online && it.id !in before }.map { it.id }.toSet()
                for ((id, sub) in subs) {
                    if (sub.ended && sub.workspace in cameBack) {
                        sub.ended = false
                        webSocket.send(encode(ClientFrame.Subscribe(Subscribe(id, sub.workspace, sub.topic))))
                    }
                }
            }
            is ServerFrame.State -> _states.value = _states.value + (frame.content.workspace to frame.content.state)
            is ServerFrame.Notification -> _notifications.tryEmit(frame.content)
            is ServerFrame.Goodbye -> {
                socket = null
                webSocket.close(1000, null)
                closedLocked(frame.content)
            }
        }
    }

    private fun lost(webSocket: WebSocket, reason: String) {
        synchronized(lock) {
            if (socket !== webSocket) return
            socket = null
            welcomed = false
            failPending(ApiError(ErrorCode.Offline, "connection lost"))
            if (!running) {
                _connection.value = Connection.Idle
                return
            }
            val wait = backoffMs(attempt)
            _connection.value = Connection.Offline(reason, System.currentTimeMillis() + wait)
            retryJob = scope.launch {
                delay(wait)
                synchronized(lock) {
                    if (running && socket == null) open()
                }
            }
        }
    }

    private fun closed(webSocket: WebSocket, error: ApiError) {
        synchronized(lock) {
            if (socket !== webSocket) return
            socket = null
            closedLocked(error)
        }
    }

    private fun closedLocked(error: ApiError) {
        welcomed = false
        running = false
        retryJob?.cancel()
        failPending(error)
        _connection.value = Connection.Closed(error)
    }

    private fun failPending(error: ApiError) {
        for (answer in pending.values) answer.complete(Response(0u, null, error))
        pending.clear()
    }

    private fun encode(frame: ClientFrame): String = ShedJson.encodeToString(ClientFrame.serializer(), frame)

    companion object {
        private const val TAG = "NexusSocket"

        /** 1 s, 2 s, 4 s… up to 30 s, give or take a fifth. */
        fun defaultBackoff(attempt: Int): Long {
            val base = (1000L shl (attempt - 1).coerceIn(0, 5)).coerceAtMost(30_000L)
            return base + Random.nextLong(base / 5 + 1)
        }
    }
}
