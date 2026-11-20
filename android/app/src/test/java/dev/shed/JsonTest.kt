package dev.shed

import dev.shed.api.AiActivity
import dev.shed.api.AiEntry
import dev.shed.api.AiRunState
import dev.shed.api.AiTimerKind
import dev.shed.api.Call
import dev.shed.api.ClientFrame
import dev.shed.api.Event
import dev.shed.api.NewTabKind
import dev.shed.api.Reply
import dev.shed.api.Request
import dev.shed.api.ServerFrame
import dev.shed.api.TabKind
import dev.shed.net.ShedJson
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The generated types read and write serde's JSON with the documented `Json` configuration. */
class JsonTest {
    private fun same(expected: String, actual: String) =
        assertEquals(Json.parseToJsonElement(expected), Json.parseToJsonElement(actual) as JsonElement)

    @Test
    fun unitVariantIsJustItsType() {
        val reply = ShedJson.decodeFromString(Reply.serializer(), """{"type":"Done"}""")
        assertEquals(Reply.Done, reply)
        same("""{"type":"Done"}""", ShedJson.encodeToString(Reply.serializer(), Reply.Done))
    }

    @Test
    fun variantWithContent() {
        val reply = ShedJson.decodeFromString(Reply.serializer(), """{"type":"Bench","content":"b-1"}""")
        assertEquals(Reply.Bench("b-1"), reply)
        same("""{"type":"Bench","content":"b-1"}""", ShedJson.encodeToString(Reply.serializer(), reply))
    }

    @Test
    fun requestFrameAsTheNexusReadsIt() {
        val frame = ClientFrame.Request(Request(7u, "ws-1", Call.NewTab(dev.shed.api.NewTab(dev.shed.api.BenchRef.Running("b"), NewTabKind.Ai(dev.shed.api.Folder("/home/you/project"))))))
        same(
            """{"type":"Request","content":{"id":7,"workspace":"ws-1","call":{"type":"NewTab","content":
               {"bench":{"type":"Running","content":"b"},"kind":{"type":"Ai","content":{"folder":"/home/you/project"}}}}}}""",
            ShedJson.encodeToString(ClientFrame.serializer(), frame),
        )
    }

    @Test
    fun optionalFieldsLeftOutAreNotWritten() {
        val call = Call.AiSettings(dev.shed.api.AiSettings(tab = "t", mode = "plan"))
        same("""{"type":"AiSettings","content":{"tab":"t","mode":"plan"}}""", ShedJson.encodeToString(Call.serializer(), call))
    }

    @Test
    fun welcomeWithAState() {
        val text = """
            {"type":"Welcome","content":{"api":1,"nexus":"0.1.0",
             "workspaces":[{"id":"w1","name":"desk","online":true,"seen":1763500000,"policy":"full","version":"0.1.0"}],
             "states":[{"workspace":"w1","state":{"host":"desk","home":"/home/you",
               "benches":[{"id":"b1","name":"shed","color":"#5CB2FF","cwd":"/home/you/shed","activeTab":"t2",
                 "tabs":[
                   {"id":"t1","title":"bash","kind":{"type":"Terminal"},"panes":[{"id":"p1","cwd":"/home/you","cols":80,"rows":24}],
                    "layout":{"type":"Pane","content":"p1"},"divider":"none","somethingNew":42},
                   {"id":"t2","title":"Claude","kind":{"type":"Ai","content":{"folder":"/home/you/shed","mode":"auto","activity":"waiting",
                     "drafts":[{"session":"s1","text":"later"}],"queued":["next"]}}}
                 ]}],
               "closed":[{"name":"old","color":"coral","cwd":"/tmp","tabs":[],"saved":1763000000}],
               "limits":{"fiveHour":{"used":0.42,"resets":1763510000},"seen":1763500000}}}]}}
        """
        val frame = ShedJson.decodeFromString(ServerFrame.serializer(), text) as ServerFrame.Welcome
        val welcome = frame.content
        assertEquals("desk", welcome.workspaces.single().name)
        val state = welcome.states.single().state
        val bench = state.benches.single()
        assertEquals(TabKind.Terminal, bench.tabs[0].kind)
        val ai = (bench.tabs[1].kind as TabKind.Ai).content
        assertEquals(AiActivity.Waiting, ai.activity)
        assertEquals(listOf("next"), ai.queued)
        assertEquals(0.42f, state.limits!!.fiveHour!!.used)
        assertNull(state.limits!!.week)
        assertEquals("/home/you", state.home)
    }

    @Test
    fun screenEvent() {
        val text = """
            {"type":"Event","content":{"sub":3,"event":{"type":"Screen","content":{
              "full":true,"cols":80,"rows":24,
              "lines":[{"row":0,"line":{"spans":[{"text":"you@shed:~$ ","fg":"2","bold":true},{"text":"ls"}],"wraps":false}}],
              "cursor":{"row":0,"col":14,"visible":true},"scrollback":120}}}}
        """
        val frame = ShedJson.decodeFromString(ServerFrame.serializer(), text) as ServerFrame.Event
        assertEquals(3u, frame.content.sub)
        val screen = (frame.content.event as Event.Screen).content
        assertTrue(screen.full)
        assertEquals(2, screen.lines.single().line.spans.size)
        assertEquals("2", screen.lines.single().line.spans[0].fg)
        assertEquals(true, screen.lines.single().line.spans[0].bold)
        assertNull(screen.alt)
        assertEquals(120u, screen.scrollback)
        assertFalse(screen.cursor.row != 0u)
    }

    @Test
    fun unitEventAndGoodbye() {
        val ev = ShedJson.decodeFromString(ServerFrame.serializer(), """{"type":"Event","content":{"sub":1,"event":{"type":"GitChanged"}}}""")
        assertEquals(Event.GitChanged, (ev as ServerFrame.Event).content.event)
        val bye = ShedJson.decodeFromString(ServerFrame.serializer(), """{"type":"Goodbye","content":{"code":"unauthorized","message":"signed out"}}""")
        assertEquals(dev.shed.api.ErrorCode.Unauthorized, (bye as ServerFrame.Goodbye).content.code)
    }

    @Test
    fun responseWithError() {
        val r = ShedJson.decodeFromString(ServerFrame.serializer(), """{"type":"Response","content":{"id":4,"ok":null,"error":{"code":"forbidden","message":"view only"}}}""")
        val response = (r as ServerFrame.Response).content
        assertNull(response.ok)
        assertEquals(dev.shed.api.ErrorCode.Forbidden, response.error!!.code)
    }

    @Test
    fun pushPayloadAsTheNexusSendsIt() {
        val p = ShedJson.decodeFromString(
            dev.shed.api.PushPayload.serializer(),
            """{"kind":"aiWaiting","title":"Claude asks","body":"","workspace":"w1","workspaceName":"laptop","bench":"b2","tab":"t3"}""",
        )
        assertEquals(dev.shed.api.NotificationKind.AiWaiting, p.kind)
        assertEquals(Triple("w1", "b2", "t3"), Triple(p.workspace, p.bench, p.tab))
        val bare = ShedJson.decodeFromString(dev.shed.api.PushPayload.serializer(), """{"kind":"workspaceOffline","title":"x","workspace":"w1"}""")
        assertNull(bare.tab)
    }

    @Test
    fun mouseInput() {
        val item: dev.shed.api.InputItem = dev.shed.api.InputItem.Mouse(dev.shed.api.Mouse(dev.shed.api.MouseKind.WheelUp, null, 3u, 4u))
        same(
            """{"type":"Mouse","content":{"kind":"wheelUp","col":3,"row":4}}""",
            ShedJson.encodeToString(dev.shed.api.InputItem.serializer(), item),
        )
    }

    @Test
    fun unknownEntryKeepsItsPlace() {
        val text = """
            {"type":"Event","content":{"sub":2,"event":{"type":"AiTranscript","content":{"from":0,"entries":[
              {"type":"User","content":{"text":"hi"}},
              {"type":"Sparkle","content":{"glitter":[1,2,3],"nested":{"type":"Tool"}}},
              {"type":"Rainbow"},
              {"type":"Note","content":{"kind":"sparkle","title":"Background task done","detail":"**ok**","at":1763500000}}
            ],"status":{"state":"idle","timers":[{"kind":"alarm","what":"check the build","at":1763500600,"tool":"t1"}]}}}}}
        """
        val event = (ShedJson.decodeFromString(ServerFrame.serializer(), text) as ServerFrame.Event).content.event
        val transcript = (event as Event.AiTranscript).content
        assertEquals(4, transcript.entries.size)
        assertTrue(transcript.entries[0] is AiEntry.User)
        assertEquals(AiEntry.Unknown, transcript.entries[1])
        assertEquals(AiEntry.Unknown, transcript.entries[2])
        // An enum value this version does not know, in a field with a default: null.
        val note = (transcript.entries[3] as AiEntry.Note).content
        assertNull(note.kind)
        assertEquals("Background task done", note.title)
        val timer = transcript.status.timers!!.single()
        assertNull(timer.kind)
        assertEquals(1763500600u, timer.at)
    }

    @Test
    fun unknownEventIsUnknown() {
        val frame = ShedJson.decodeFromString(
            ServerFrame.serializer(),
            """{"type":"Event","content":{"sub":5,"event":{"type":"Fireworks","content":{"bang":true}}}}""",
        )
        assertEquals(5u, (frame as ServerFrame.Event).content.sub)
        assertEquals(Event.Unknown, frame.content.event)
        val unit = ShedJson.decodeFromString(ServerFrame.serializer(), """{"type":"Event","content":{"sub":5,"event":{"type":"Quiet"}}}""")
        assertEquals(Event.Unknown, (unit as ServerFrame.Event).content.event)
    }

    @Test
    fun toolWithARunAndEntriesChanged() {
        val text = """
            {"type":"Event","content":{"sub":1,"event":{"type":"AiEntries","content":{"entries":[{"index":3,"entry":
              {"type":"Tool","content":{"id":"t1","name":"Agent","summary":"Agent: look around","run":{
                "state":"running","agent":"Explore","started":1763500000,"last":"Reading x","tools":4,"tokens":12000,
                "steps":[{"type":"Tool","content":{"id":"s1","name":"Read","summary":"Read x","result":"ok"}},
                         {"type":"Assistant","content":{"text":"Found it"}},{"type":"Hologram"}],
                "skipped":2}}}}],
              "status":{"state":"working","timers":[{"kind":"wakeup","what":"look again","at":1763500600}]}}}}}
        """
        val changed = ((ShedJson.decodeFromString(ServerFrame.serializer(), text) as ServerFrame.Event).content.event as Event.AiEntries).content
        val at = changed.entries.single()
        assertEquals(3u, at.index)
        val run = (at.entry as AiEntry.Tool).content.run!!
        assertEquals(AiRunState.Running, run.state)
        assertEquals("Explore", run.agent)
        assertEquals(3, run.steps!!.size)
        assertEquals(AiEntry.Unknown, run.steps!![2])
        assertEquals(AiTimerKind.Wakeup, changed.status.timers!!.single().kind)
    }

    @Test
    fun uploadCall() {
        val call = Call.AiUpload(dev.shed.api.AiUpload("t", "notes.txt", "aGk="))
        same("""{"type":"AiUpload","content":{"tab":"t","name":"notes.txt","data":"aGk="}}""", ShedJson.encodeToString(Call.serializer(), call))
    }
}
