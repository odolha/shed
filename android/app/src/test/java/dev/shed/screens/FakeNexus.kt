package dev.shed.screens

import dev.shed.Session
import dev.shed.api.Call
import dev.shed.api.ClientFrame
import dev.shed.api.Event
import dev.shed.api.EventFrame
import dev.shed.api.Reply
import dev.shed.api.Response
import dev.shed.api.ServerFrame
import dev.shed.api.StateFrame
import dev.shed.api.Topic
import dev.shed.api.Welcome
import dev.shed.api.WorkspaceInfo
import dev.shed.api.WorkspaceState
import dev.shed.data.Account
import dev.shed.net.NexusHttp
import dev.shed.net.NexusSocket
import dev.shed.net.ShedJson
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import okhttp3.OkHttpClient
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.mockwebserver.Dispatcher
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.RecordedRequest
import okhttp3.mockwebserver.MockWebServer

/**
 * A pretend nexus for drawing whole screens: one workspace with [state],
 * answering calls with [answer], subscriptions with [events] and HTTP
 * requests (`/api/v1/…`, by path) with [http]'s JSON. A real [NexusSocket]
 * talks to it over a local WebSocket.
 */
class FakeNexus(
    private val info: WorkspaceInfo,
    private val state: WorkspaceState,
    private val answer: (Call) -> Reply = { Reply.Done },
    private val events: (Topic) -> List<Event> = { emptyList() },
    private val http: (String) -> String? = { null },
) {
    private val server = MockWebServer()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val client = OkHttpClient()
    val session: Session

    init {
        // Every connection (and a pull to refresh's next one) is welcomed alike.
        server.dispatcher = object : Dispatcher() {
            override fun dispatch(request: RecordedRequest): MockResponse {
                val path = request.path.orEmpty().substringBefore('?')
                if (path == "/api") return MockResponse().withWebSocketUpgrade(Listener())
                val body = http(path.removePrefix("/api/v1")) ?: return MockResponse().setResponseCode(404).setBody("""{"code":"notFound","message":"no such route"}""")
                return MockResponse().setHeader("Content-Type", "application/json").setBody(body)
            }
        }
        server.start()
        val base = server.url("/").toString().trimEnd('/')
        val socket = NexusSocket(client, base, "token", scope, "test")
        session = Session(Account(base, "you", "phone", "token"), NexusHttp(client, base) { "token" }, socket)
        socket.start()
    }

    private fun send(ws: WebSocket, frame: ServerFrame) {
        ws.send(ShedJson.encodeToString(ServerFrame.serializer(), frame))
    }

    private inner class Listener : WebSocketListener() {
        override fun onMessage(webSocket: WebSocket, text: String) {
            when (val frame = ShedJson.decodeFromString(ClientFrame.serializer(), text)) {
                is ClientFrame.Hello -> send(webSocket, ServerFrame.Welcome(Welcome(1u, "0.1.0", listOf(info), listOf(StateFrame(info.id, state)))))
                is ClientFrame.Request -> send(webSocket, ServerFrame.Response(Response(frame.content.id, answer(frame.content.call), null)))
                is ClientFrame.Subscribe -> for (e in events(frame.content.topic)) send(webSocket, ServerFrame.Event(EventFrame(frame.content.id, e)))
                else -> {}
            }
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
        }
    }

    fun close() {
        session.socket.stop()
        scope.cancel()
        server.shutdown()
    }
}
