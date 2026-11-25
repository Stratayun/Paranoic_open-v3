package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowLeft
import androidx.compose.material.icons.rounded.*
import androidx.compose.material3.Icon
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.dp

enum class AppSymbol {
    Home, Settings, Back, Folder, Document, Cloud, Security, Vault, Recovery, Search, Analytics, Trash, Sync, Passwords,
    Photo, Video, Audio, Archive, Favorite, Recent, More,
    Menu, Forward, Add, Download, Camera, Phone, SdCard, Tasks, Notes, Server, Sort, ViewList, ViewGrid, Up, Close, Info, Edit,
    Copy, MoveTo, SelectAll, NewFolder, NewFile, Refresh, Cut, Swap, ZoomIn, ZoomOut, Prev, Save, Fullscreen,
}

/** Material Symbols (Rounded, Apache-2.0). Decorative: the labelled control owns accessibility. */
fun AppSymbol.vector(): ImageVector = when (this) {
    AppSymbol.Home -> Icons.Rounded.Home
    AppSymbol.Settings -> Icons.Rounded.Settings
    AppSymbol.Back -> Icons.AutoMirrored.Rounded.ArrowBack
    AppSymbol.Folder -> Icons.Rounded.Folder
    AppSymbol.Document -> Icons.Rounded.Description
    AppSymbol.Cloud -> Icons.Rounded.Cloud
    AppSymbol.Security -> Icons.Rounded.Shield
    AppSymbol.Vault -> Icons.Rounded.Lock
    AppSymbol.Recovery -> Icons.Rounded.Restore
    AppSymbol.Search -> Icons.Rounded.Search
    AppSymbol.Analytics -> Icons.Rounded.Analytics
    AppSymbol.Trash -> Icons.Rounded.Delete
    AppSymbol.Sync -> Icons.Rounded.Sync
    AppSymbol.Passwords -> Icons.Rounded.Key
    AppSymbol.Photo -> Icons.Rounded.Image
    AppSymbol.Video -> Icons.Rounded.Movie
    AppSymbol.Audio -> Icons.Rounded.MusicNote
    AppSymbol.Archive -> Icons.Rounded.Archive
    AppSymbol.Favorite -> Icons.Rounded.Star
    AppSymbol.Recent -> Icons.Rounded.History
    AppSymbol.More -> Icons.Rounded.MoreVert
    AppSymbol.Menu -> Icons.Rounded.Menu
    AppSymbol.Forward -> Icons.AutoMirrored.Rounded.KeyboardArrowRight
    AppSymbol.Add -> Icons.Rounded.Add
    AppSymbol.Download -> Icons.Rounded.Download
    AppSymbol.Camera -> Icons.Rounded.PhotoCamera
    AppSymbol.Phone -> Icons.Rounded.PhoneAndroid
    AppSymbol.SdCard -> Icons.Rounded.SdCard
    AppSymbol.Tasks -> Icons.Rounded.TaskAlt
    AppSymbol.Notes -> Icons.Rounded.EditNote
    AppSymbol.Server -> Icons.Rounded.Dns
    AppSymbol.Sort -> Icons.Rounded.SortByAlpha
    AppSymbol.ViewList -> Icons.Rounded.ViewAgenda
    AppSymbol.ViewGrid -> Icons.Rounded.GridView
    AppSymbol.Up -> Icons.Rounded.ArrowUpward
    AppSymbol.Close -> Icons.Rounded.Close
    AppSymbol.Info -> Icons.Rounded.Info
    AppSymbol.Edit -> Icons.Rounded.Edit
    AppSymbol.Copy -> Icons.Rounded.ContentCopy
    AppSymbol.MoveTo -> Icons.Rounded.DriveFileMove
    AppSymbol.SelectAll -> Icons.Rounded.SelectAll
    AppSymbol.NewFolder -> Icons.Rounded.CreateNewFolder
    AppSymbol.NewFile -> Icons.Rounded.NoteAdd
    AppSymbol.Refresh -> Icons.Rounded.Refresh
    AppSymbol.Cut -> Icons.Rounded.ContentCut
    AppSymbol.Swap -> Icons.Rounded.SwapHoriz
    AppSymbol.ZoomIn -> Icons.Rounded.ZoomIn
    AppSymbol.ZoomOut -> Icons.Rounded.ZoomOut
    AppSymbol.Prev -> Icons.AutoMirrored.Rounded.KeyboardArrowLeft
    AppSymbol.Save -> Icons.Rounded.Save
    AppSymbol.Fullscreen -> Icons.Rounded.Fullscreen
}

@Composable
fun AppIcon(symbol: AppSymbol, modifier: Modifier = Modifier) {
    Icon(symbol.vector(), contentDescription = null, modifier = modifier.size(24.dp))
}
