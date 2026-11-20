package dev.shed.net

import dev.shed.api.AiEntry
import dev.shed.api.ApiError
import dev.shed.api.ErrorCode
import dev.shed.api.Event
import kotlinx.serialization.json.Json
import kotlinx.serialization.modules.SerializersModule

/**
 * The JSON the nexus speaks (see the header of `dev.shed.api.Api`). Lenient
 * with what a newer workspace may send: an entry or event this version does
 * not know reads as `AiEntry.Unknown` / `Event.Unknown`, and an enum value it
 * does not know, in a field with a default, as that default (null).
 */
val ShedJson = Json {
    classDiscriminator = "type"
    ignoreUnknownKeys = true
    coerceInputValues = true
    serializersModule = SerializersModule {
        polymorphicDefaultDeserializer(AiEntry::class) { AiEntry.Unknown.serializer() }
        polymorphicDefaultDeserializer(Event::class) { Event.Unknown.serializer() }
    }
}

/** A call or HTTP request that the nexus or a workspace answered with an error. */
class ApiException(val error: ApiError, val status: Int = 0) : Exception("${error.code.string}: ${error.message}") {
    val code: ErrorCode get() = error.code

    companion object {
        fun of(code: ErrorCode, message: String = "") = ApiException(ApiError(code, message))
    }
}
