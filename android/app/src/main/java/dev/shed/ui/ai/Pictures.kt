package dev.shed.ui.ai

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.net.Uri
import android.util.Base64
import androidx.core.content.FileProvider
import androidx.exifinterface.media.ExifInterface
import dev.shed.api.Picture
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.ByteArrayOutputStream
import java.io.File
import kotlin.math.max
import kotlin.math.roundToInt

/** The longest side a picture sent with a prompt may have. */
const val MAX_PICTURE_SIDE = 2048

/**
 * A photo or picture for a prompt: turned upright, made no larger than
 * [MAX_PICTURE_SIDE] on its longest side and sent as JPEG, to keep frames small.
 */
suspend fun pictureForPrompt(context: Context, uri: Uri): Picture? = withContext(Dispatchers.IO) {
    val resolver = context.contentResolver
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) } ?: return@withContext null
    val longest = max(bounds.outWidth, bounds.outHeight)
    if (longest <= 0) return@withContext null
    var sample = 1
    while (longest / (sample * 2) >= MAX_PICTURE_SIDE) sample *= 2
    val options = BitmapFactory.Options().apply { inSampleSize = sample }
    var bitmap = resolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, options) } ?: return@withContext null
    val scale = MAX_PICTURE_SIDE.toFloat() / max(bitmap.width, bitmap.height)
    if (scale < 1f) {
        bitmap = Bitmap.createScaledBitmap(bitmap, (bitmap.width * scale).roundToInt(), (bitmap.height * scale).roundToInt(), true)
    }
    val rotation = runCatching {
        resolver.openInputStream(uri)?.use { ExifInterface(it).rotationDegrees } ?: 0
    }.getOrDefault(0)
    if (rotation != 0) {
        bitmap = Bitmap.createBitmap(bitmap, 0, 0, bitmap.width, bitmap.height, Matrix().apply { postRotate(rotation.toFloat()) }, true)
    }
    val out = ByteArrayOutputStream()
    bitmap.compress(Bitmap.CompressFormat.JPEG, 85, out)
    Picture("image/jpeg", Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP))
}

/** Where the camera writes a new photo (shared through the FileProvider). */
fun cameraUri(context: Context): Uri {
    val dir = File(context.cacheDir, "camera").apply { mkdirs() }
    dir.listFiles()?.forEach { it.delete() }
    val file = File(dir, "photo-${System.currentTimeMillis()}.jpg")
    return FileProvider.getUriForFile(context, context.packageName + ".files", file)
}
