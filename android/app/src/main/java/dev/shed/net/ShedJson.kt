package dev.shed.net

import dev.shed.api.ApiError
import dev.shed.api.ErrorCode
import kotlinx.serialization.json.Json

/** The JSON the nexus speaks (see the header of `dev.shed.api.Api`). */
val ShedJson = Json {
    classDiscriminator = "type"
    ignoreUnknownKeys = true
}

/** A call or HTTP request that the nexus or a workspace answered with an error. */
class ApiException(val error: ApiError, val status: Int = 0) : Exception("${error.code.string}: ${error.message}") {
    val code: ErrorCode get() = error.code

    companion object {
        fun of(code: ErrorCode, message: String = "") = ApiException(ApiError(code, message))
    }
}
