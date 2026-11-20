package dev.shed.ui.common

import android.graphics.BitmapFactory
import android.util.Base64
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.shed.R
import dev.shed.api.Diff
import dev.shed.api.DiffLineKind
import dev.shed.api.Hunk
import dev.shed.api.Picture
import dev.shed.ui.theme.LocalShed
import dev.shed.ui.theme.Mono

/** A picture sent as base64, as a bitmap (`null` if it does not decode). */
fun decodePicture(picture: Picture): ImageBitmap? = runCatching {
    val bytes = Base64.decode(picture.data, Base64.DEFAULT)
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
}.getOrNull()

@Composable
fun PictureView(picture: Picture, modifier: Modifier = Modifier) {
    val bitmap = remember(picture) { decodePicture(picture) }
    if (bitmap != null) {
        Image(bitmap, null, modifier.heightIn(max = 360.dp), contentScale = ContentScale.Fit)
    }
}

/**
 * A diff, hunk by hunk, with line numbers; [hunkActions] draws buttons in
 * each hunk's header (stage, discard…).
 */
@Composable
fun DiffView(
    diff: Diff,
    modifier: Modifier = Modifier,
    hunkActions: (@Composable (Int, Hunk) -> Unit)? = null,
) {
    val shed = LocalShed.current
    Column(modifier.fillMaxWidth()) {
        if (diff.binary == true) {
            Text(stringResource(R.string.diff_binary), color = shed.textDim, modifier = Modifier.padding(8.dp))
        }
        if (diff.oldPicture != null || diff.newPicture != null) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(8.dp)) {
                diff.oldPicture?.let { Column(Modifier.weight(1f)) { Text("−", color = shed.removed); PictureView(it) } }
                diff.newPicture?.let { Column(Modifier.weight(1f)) { Text("+", color = shed.added); PictureView(it) } }
            }
        }
        val scroll = rememberScrollState()
        diff.hunks.forEachIndexed { index, hunk ->
            Row(
                Modifier.fillMaxWidth().background(shed.hover).padding(horizontal = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(hunk.header, fontFamily = Mono, fontSize = 11.sp, color = shed.textDim, maxLines = 1, modifier = Modifier.weight(1f))
                hunkActions?.invoke(index, hunk)
            }
            Column(Modifier.fillMaxWidth().horizontalScroll(scroll)) {
                for (line in hunk.lines) {
                    val (bg, sign, fg) = when (line.kind) {
                        DiffLineKind.Added -> Triple(shed.addedBackground, "+", shed.text)
                        DiffLineKind.Removed -> Triple(shed.removedBackground, "−", shed.text)
                        DiffLineKind.Context -> Triple(Color.Transparent, " ", shed.text)
                    }
                    Row(Modifier.background(bg)) {
                        Text(
                            (line.old?.toString() ?: "").padStart(4),
                            fontFamily = Mono, fontSize = 11.sp, color = shed.textDim,
                            modifier = Modifier.width(34.dp),
                        )
                        Text(
                            (line.new?.toString() ?: "").padStart(4),
                            fontFamily = Mono, fontSize = 11.sp, color = shed.textDim,
                            modifier = Modifier.width(34.dp),
                        )
                        Text(
                            "$sign ${line.text}",
                            fontFamily = Mono,
                            fontSize = 12.sp,
                            color = fg,
                            softWrap = false,
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                }
            }
        }
    }
}
