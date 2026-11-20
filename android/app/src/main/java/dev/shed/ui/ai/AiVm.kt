package dev.shed.ui.ai

import androidx.compose.ui.graphics.ImageBitmap
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.shed.api.AiEntry
import dev.shed.api.AiInfo
import dev.shed.api.AiPictureName
import dev.shed.api.AiSession
import dev.shed.api.AiStatus
import dev.shed.api.AiTopic
import dev.shed.api.AiTranscript
import dev.shed.api.Call
import dev.shed.api.Event
import dev.shed.api.Reply
import dev.shed.api.TabOnly
import dev.shed.api.Topic
import dev.shed.net.NexusSocket
import dev.shed.net.SubEvent
import dev.shed.ui.common.decodePicture
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

/** A conversation as a list: `AiTranscript` events replace entries from `from` on. */
fun applyTranscript(entries: List<AiEntry>, update: AiTranscript): List<AiEntry> =
    entries.take(update.from.toInt().coerceAtMost(entries.size)) + update.entries

/** An AI tab while it is shown: its transcript, status, what Claude Code offers, sessions, pictures. */
class AiVm(private val socket: NexusSocket, private val ws: String, val tab: String) : ViewModel() {
    private val _entries = MutableStateFlow<List<AiEntry>?>(null)
    val entries: StateFlow<List<AiEntry>?> = _entries
    private val _status = MutableStateFlow<AiStatus?>(null)
    val status: StateFlow<AiStatus?> = _status
    private val _info = MutableStateFlow(AiInfo(emptyList(), emptyList()))
    val info: StateFlow<AiInfo> = _info
    private val _sessions = MutableStateFlow<List<AiSession>?>(null)
    val sessions: StateFlow<List<AiSession>?> = _sessions
    private val _ended = MutableStateFlow(false)
    val ended: StateFlow<Boolean> = _ended
    private val _pictures = MutableStateFlow<Map<String, ImageBitmap?>>(emptyMap())
    val pictures: StateFlow<Map<String, ImageBitmap?>> = _pictures

    init {
        viewModelScope.launch {
            socket.subscribe(ws, Topic.Ai(AiTopic(tab))).collect { ev ->
                when (ev) {
                    is SubEvent.Data -> when (val e = ev.event) {
                        is Event.AiTranscript -> {
                            val before = _status.value?.session
                            _entries.value = applyTranscript(_entries.value.orEmpty(), e.content)
                            _status.value = e.content.status
                            _ended.value = false
                            if (before != e.content.status.session) loadSessions()
                        }
                        is Event.AiInfo -> _info.value = e.content
                        else -> {}
                    }
                    is SubEvent.Ended -> _ended.value = true
                }
            }
        }
        loadSessions()
    }

    fun loadSessions() {
        viewModelScope.launch {
            runCatching {
                (socket.call(ws, Call.AiSessions(TabOnly(tab))) as? Reply.Sessions)?.content
            }.getOrNull()?.let { _sessions.value = it }
        }
    }

    /** Fetches a picture of the conversation once. */
    fun picture(name: String) {
        if (name in _pictures.value) return
        _pictures.value = _pictures.value + (name to null)
        viewModelScope.launch {
            val bitmap = runCatching {
                (socket.call(ws, Call.AiPicture(AiPictureName(tab, name))) as? Reply.Picture)?.content?.let(::decodePicture)
            }.getOrNull()
            _pictures.value = _pictures.value + (name to bitmap)
        }
    }
}
