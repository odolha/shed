package dev.shed.net

import dev.shed.api.ApiError
import dev.shed.api.ErrorCode
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer
import okhttp3.Call
import okhttp3.Callback
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import java.io.IOException
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/** `{}`: what the nexus answers when there is nothing to say. */
@Serializable
class Empty

/**
 * The nexus's HTTP API (`/api/v1/…`, see `crates/shed-api/src/http.rs`), with
 * the phone's device token as `Authorization: Bearer` once signed in.
 */
class NexusHttp(
    private val client: OkHttpClient,
    private val base: String,
    private val token: () -> String?,
) {
    suspend fun <T> get(path: String, answer: KSerializer<T>): T =
        send(request(path).get().build(), answer)

    suspend fun <B, T> post(path: String, body: B, bodySerializer: KSerializer<B>, answer: KSerializer<T>): T {
        val json = ShedJson.encodeToString(bodySerializer, body)
        return send(request(path).post(json.toRequestBody(JSON)).build(), answer)
    }

    suspend fun <T> post(path: String, answer: KSerializer<T>): T =
        send(request(path).post("{}".toRequestBody(JSON)).build(), answer)

    suspend fun <T> getList(path: String, item: KSerializer<T>): List<T> = get(path, ListSerializer(item))

    private fun request(path: String): Request.Builder {
        val builder = Request.Builder().url(base.trimEnd('/') + "/api/v1" + path)
        token()?.let { builder.header("Authorization", "Bearer $it") }
        return builder
    }

    private suspend fun <T> send(request: Request, answer: KSerializer<T>): T {
        val response = client.newCall(request).await()
        response.use {
            val text = it.body?.string().orEmpty()
            if (!it.isSuccessful) {
                val error = runCatching { ShedJson.decodeFromString(ApiError.serializer(), text) }.getOrNull()
                    ?: ApiError(codeFor(it.code), "HTTP ${it.code}")
                throw ApiException(error, it.code)
            }
            return try {
                ShedJson.decodeFromString(answer, text.ifEmpty { "{}" })
            } catch (e: IllegalArgumentException) {
                throw ApiException(ApiError(ErrorCode.Failed, "the nexus's answer does not read: ${e.message}"), it.code)
            }
        }
    }

    companion object {
        private val JSON = "application/json".toMediaType()

        fun codeFor(status: Int): ErrorCode = when (status) {
            401 -> ErrorCode.Unauthorized
            403 -> ErrorCode.Forbidden
            404 -> ErrorCode.NotFound
            429 -> ErrorCode.RateLimited
            503 -> ErrorCode.Offline
            504 -> ErrorCode.Timeout
            in 400..499 -> ErrorCode.Invalid
            else -> ErrorCode.Failed
        }

        /**
         * A nexus address as typed, made whole: `https://` added when no scheme
         * is given, no trailing slash. Plain `http` only for this machine during
         * development (the emulator's `10.0.2.2`, `localhost`); `null` otherwise.
         */
        fun normalizeUrl(typed: String): String? {
            var text = typed.trim().trimEnd('/')
            if (text.isEmpty()) return null
            if (!text.contains("://")) text = "https://$text"
            val url = text.toHttpUrlOrNull() ?: return null
            if (url.scheme == "http" && url.host !in DEV_HOSTS) return null
            if (url.scheme != "https" && url.scheme != "http") return null
            return text
        }

        private val DEV_HOSTS = setOf("10.0.2.2", "localhost", "127.0.0.1")
    }
}

/** Waits for an OkHttp call, cancelling it with the coroutine. */
suspend fun Call.await(): Response = suspendCancellableCoroutine { cont ->
    enqueue(object : Callback {
        override fun onResponse(call: Call, response: Response) = cont.resume(response)
        override fun onFailure(call: Call, e: IOException) {
            if (!cont.isCancelled) cont.resumeWithException(e)
        }
    })
    cont.invokeOnCancellation { runCatching { cancel() } }
}
