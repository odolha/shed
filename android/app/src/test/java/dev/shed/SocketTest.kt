package dev.shed

import dev.shed.api.AiTopic
import dev.shed.api.ApiError
import dev.shed.api.Call
import dev.shed.api.ClientFrame
import dev.shed.api.ErrorCode
import dev.shed.api.Event
import dev.shed.api.EventFrame
import dev.shed.api.Notification
import dev.shed.api.NotificationFrame
import dev.shed.api.NotificationKind
import dev.shed.api.Policy
import dev.shed.api.Reply
import dev.shed.api.Response
import dev.shed.api.ServerFrame
import dev.shed.api.Topic
import dev.shed.api.Welcome
import dev.shed.api.WorkspaceInfo
import dev.shed.net.ApiException
import dev.shed.net.Connection
import dev.shed.net.NexusSocket
import dev.shed.net.ShedJson
import dev.shed.net.SubEvent
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import okhttp3.OkHttpClient
import okhttp3.Response as HttpResponse
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/** `NexusSocket` against a fake nexus (MockWebServer's WebSocket). */
class SocketTest {
    private lateinit var server: MockWebServer
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private val client = OkHttpClient()

    /** A fake nexus connection: welcomes, answers requests, records what came. */
    private inner class Fake(private val onRequest: (WebSocket, ClientFrame.Request) -> Unit = ::pong) : WebSocketListener() {
        val frames = LinkedBlockingQueue<ClientFrame>()
        /** `Visible` frames, kept apart from the rest. */
        val visible = LinkedBlockingQueue<Boolean>()
        @Volatile var socket: WebSocket? = null

        override fun onOpen(webSocket: WebSocket, response: HttpResponse) {
            socket = webSocket
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            val frame = ShedJson.decodeFromString(ClientFrame.serializer(), text)
            if (frame is ClientFrame.Visible) {
                visible.add(frame.content)
                return
            }
            frames.add(frame)
            when (frame) {
                is ClientFrame.Hello -> send(webSocket, ServerFrame.Welcome(Welcome(1u, "0.1.0", listOf(WS), emptyList())))
                is ClientFrame.Request -> onRequest(webSocket, frame)
                else -> {}
            }
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
        }

        fun next(): ClientFrame = frames.poll(5, TimeUnit.SECONDS) ?: error("nothing came")
    }

    private fun send(ws: WebSocket, frame: ServerFrame) {
        ws.send(ShedJson.encodeToString(ServerFrame.serializer(), frame))
    }

    private fun pong(ws: WebSocket, req: ClientFrame.Request) {
        send(ws, ServerFrame.Response(Response(req.content.id, Reply.Text("pong ${req.content.workspace}"), null)))
    }

    @Before
    fun setUp() {
        server = MockWebServer()
        server.start()
    }

    @After
    fun tearDown() {
        scope.cancel()
        server.shutdown()
    }

    private fun socket(timeout: Long = 5_000) =
        NexusSocket(client, server.url("/").toString().trimEnd('/'), "secret", scope, "test", timeout) { 50L }

    @Test
    fun helloThenRequestAndAnswer() = runBlocking {
        val fake = Fake()
        server.enqueue(MockResponse().withWebSocketUpgrade(fake))
        val socket = socket()
        socket.start()
        val reply = withTimeout(5_000) { socket.call("w1", Call.Previews) }
        assertEquals(Reply.Text("pong w1"), reply)
        val upgrade = server.takeRequest()
        assertEquals("/api", upgrade.path)
        assertEquals("Bearer secret", upgrade.getHeader("Authorization"))
        assertTrue(fake.next() is ClientFrame.Hello)
        assertEquals("w1", (socket.workspaces.value.single()).id)
        socket.stop()
    }

    @Test
    fun errorsComeBackAsApiExceptions() = runBlocking {
        val fake = Fake { ws, req ->
            send(ws, ServerFrame.Response(Response(req.content.id, null, ApiError(ErrorCode.Forbidden, "view only"))))
        }
        server.enqueue(MockResponse().withWebSocketUpgrade(fake))
        val socket = socket()
        socket.start()
        try {
            socket.call("w1", Call.Previews)
            fail("expected an error")
        } catch (e: ApiException) {
            assertEquals(ErrorCode.Forbidden, e.code)
        }
        socket.stop()
    }

    @Test
    fun unansweredRequestTimesOut() = runBlocking {
        val fake = Fake { _, _ -> }
        server.enqueue(MockResponse().withWebSocketUpgrade(fake))
        val socket = socket(timeout = 500)
        socket.start()
        try {
            socket.call("w1", Call.Previews)
            fail("expected a timeout")
        } catch (e: ApiException) {
            assertEquals(ErrorCode.Timeout, e.code)
        }
        socket.stop()
    }

    @Test
    fun subscriptionsComeBackAfterAReconnect() = runBlocking {
        val first = Fake()
        val second = Fake()
        server.enqueue(MockResponse().withWebSocketUpgrade(first))
        server.enqueue(MockResponse().withWebSocketUpgrade(second))
        val socket = socket()
        socket.start()
        val topic = Topic.Ai(AiTopic("t1"))
        val events = async(Dispatchers.Default) { socket.subscribe("w1", topic).take(2).toList() }

        assertTrue(first.next() is ClientFrame.Hello)
        val sub = first.next() as ClientFrame.Subscribe
        assertEquals(topic, sub.content.topic)
        send(first.socket!!, ServerFrame.Event(EventFrame(sub.content.id, Event.GitChanged)))

        // The nexus goes away; the socket dials again and subscribes again, same id.
        first.socket!!.close(1001, "restart")
        val hello = second.frames.poll(5, TimeUnit.SECONDS)
        assertTrue("reconnected: ${socket.connection.value}, requests ${server.requestCount}", hello is ClientFrame.Hello)
        val again = second.next() as ClientFrame.Subscribe
        assertEquals(sub.content.id, again.content.id)
        assertEquals(topic, again.content.topic)
        send(second.socket!!, ServerFrame.Event(EventFrame(again.content.id, Event.GitChanged)))

        val got = withTimeout(5_000) { events.await() }
        assertEquals(listOf(SubEvent.Data(Event.GitChanged), SubEvent.Data(Event.GitChanged)), got)
        socket.stop()
    }

    @Test
    fun goodbyeClosesForGood() = runBlocking {
        val fake = object : WebSocketListener() {
            override fun onMessage(webSocket: WebSocket, text: String) {
                send(webSocket, ServerFrame.Goodbye(ApiError(ErrorCode.Unauthorized, "revoked")))
                webSocket.close(1000, null)
            }
        }
        server.enqueue(MockResponse().withWebSocketUpgrade(fake))
        val socket = socket()
        socket.start()
        val closed = withTimeout(5_000) { socket.connection.first { it is Connection.Closed } } as Connection.Closed
        assertEquals(ErrorCode.Unauthorized, closed.error.code)
        assertEquals(1, server.requestCount)
    }

    @Test
    fun unknownEventsAreLeftOut() = runBlocking {
        val fake = Fake()
        server.enqueue(MockResponse().withWebSocketUpgrade(fake))
        val socket = socket()
        socket.start()
        val events = async(Dispatchers.Default) { socket.subscribe("w1", Topic.Ai(AiTopic("t1"))).take(1).toList() }
        assertTrue(fake.next() is ClientFrame.Hello)
        val sub = fake.next() as ClientFrame.Subscribe
        // A newer workspace's event, then one this version knows.
        fake.socket!!.send("""{"type":"Event","content":{"sub":${sub.content.id},"event":{"type":"Fireworks","content":{"bang":true}}}}""")
        send(fake.socket!!, ServerFrame.Event(EventFrame(sub.content.id, Event.GitChanged)))
        assertEquals(listOf(SubEvent.Data(Event.GitChanged)), withTimeout(5_000) { events.await() })
        assertEquals(Connection.Online, socket.connection.value)
        socket.stop()
    }

    @Test
    fun saysWhetherInViewAndGetsLiveNotifications() = runBlocking {
        val first = Fake()
        val second = Fake()
        server.enqueue(MockResponse().withWebSocketUpgrade(first))
        server.enqueue(MockResponse().withWebSocketUpgrade(second))
        val socket = socket()
        socket.setVisible(true)
        socket.start()
        // After the welcome, as it is; then on every change only.
        assertEquals(true, first.visible.poll(5, TimeUnit.SECONDS))
        socket.setVisible(true)
        socket.setVisible(false)
        assertEquals(false, first.visible.poll(5, TimeUnit.SECONDS))

        val frame = NotificationFrame("w1", Notification(NotificationKind.AiDone, "Claude is done", "api · claude", "b1", "t1"))
        val live = async(Dispatchers.Default) { socket.notifications.first() }
        // Collecting has started by the time the welcome is in.
        withTimeout(5_000) { socket.connection.first { it == Connection.Online } }
        delay(100)
        send(first.socket!!, ServerFrame.Notification(frame))
        assertEquals(frame, withTimeout(5_000) { live.await() })

        // A new connection hears how it is now.
        first.socket!!.close(1001, "restart")
        assertEquals(false, second.visible.poll(5, TimeUnit.SECONDS))
        socket.stop()
    }

    private companion object {
        val WS = WorkspaceInfo("w1", "desk", true, 1u, Policy.Full, "0.1.0")
    }
}
