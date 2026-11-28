package io.github.stratayun.paranoic.ui.components

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.stratayun.paranoic.ui.state.actionDescription
import io.github.stratayun.paranoic.ui.state.tr
import org.jetbrains.compose.resources.DrawableResource

/*
 * Building blocks of the owner's interface concept (27.11): a section header with a large glass
 * icon, rounded cards with a hairline border, tile actions, a pill search field and empty states
 * with one main button. Screens compose these instead of bare text buttons.
 */

/** Section header: large icon, title and one line of explanation. */
@Composable
fun SectionHero(icon: DrawableResource, title: String, subtitle: String, modifier: Modifier = Modifier, iconSize: Dp = 72.dp) {
    Row(modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        BrandIcon(icon, size = iconSize)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.SemiBold, maxLines = 2,
                overflow = TextOverflow.Ellipsis)
            Text(subtitle, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 3,
                overflow = TextOverflow.Ellipsis)
        }
    }
}

/** Rounded card with a thin border, the container of every group in the concept. */
@Composable
fun ConceptCard(modifier: Modifier = Modifier, padding: Dp = 12.dp, content: @Composable ColumnScope.() -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Surface(modifier.fillMaxWidth(), shape = RoundedCornerShape(24.dp), color = scheme.surfaceContainer,
        border = BorderStroke(1.dp, scheme.outlineVariant)) {
        Column(Modifier.padding(padding), verticalArrangement = Arrangement.spacedBy(8.dp), content = content)
    }
}

/** Card title with an optional action on the right ("Показать все ›"). */
@Composable
fun ConceptCardTitle(title: String, action: String? = null, onAction: (() -> Unit)? = null) {
    Row(Modifier.fillMaxWidth().padding(start = 8.dp, top = 2.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(title, Modifier.weight(1f), style = MaterialTheme.typography.titleMedium.copy(fontSize = 18.sp),
            fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (action != null && onAction != null) TextButton(onClick = onAction) {
            Text(action, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurface)
            Spacer(Modifier.width(4.dp))
            Icon(AppSymbol.Forward.vector(), null, Modifier.size(18.dp))
        }
    }
}

/** Row inside a card: icon, title, subtitle and a trailing part (chevron by default). */
@Composable
fun ConceptRow(title: String, subtitle: String? = null, icon: DrawableResource? = null, modifier: Modifier = Modifier,
    enabled: Boolean = true, trailing: (@Composable () -> Unit)? = null, onClick: (() -> Unit)? = null) {
    val scheme = MaterialTheme.colorScheme
    Surface(modifier.fillMaxWidth(), shape = RoundedCornerShape(18.dp), color = scheme.surfaceContainerHigh) {
        Row((if (onClick != null) Modifier.clickable(enabled = enabled, role = Role.Button, onClick = onClick) else Modifier)
            .padding(horizontal = 12.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
            if (icon != null) { BrandIcon(icon, size = 44.dp); Spacer(Modifier.width(12.dp)) }
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis,
                    color = if (enabled) scheme.onSurface else scheme.onSurfaceVariant)
                if (subtitle != null) Text(subtitle, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant,
                    maxLines = 3, overflow = TextOverflow.Ellipsis)
            }
            Spacer(Modifier.width(8.dp))
            if (trailing != null) trailing()
            else if (onClick != null) Icon(AppSymbol.Forward.vector(), null, Modifier.size(22.dp), tint = scheme.onSurfaceVariant)
        }
    }
}

/** Square action tile (concept: "Добавить папку", "Сервер WebDAV", "Обзор хранилищ"). */
@Composable
fun ActionTile(label: String, symbol: AppSymbol, modifier: Modifier = Modifier, enabled: Boolean = true, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Surface(modifier.heightIn(min = 84.dp), shape = RoundedCornerShape(20.dp), color = scheme.surfaceContainerHigh,
        border = BorderStroke(1.dp, scheme.outlineVariant)) {
        Column(Modifier.clickable(enabled = enabled, role = Role.Button, onClick = onClick).padding(horizontal = 8.dp, vertical = 12.dp),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(symbol.vector(), null, Modifier.size(26.dp), tint = if (enabled) scheme.onSurface else scheme.onSurfaceVariant)
            Text(label, style = MaterialTheme.typography.labelMedium, textAlign = TextAlign.Center, maxLines = 2,
                color = if (enabled) scheme.onSurface else scheme.onSurfaceVariant)
        }
    }
}

/** A row of equally wide action tiles. */
@Composable
fun ActionTiles(vararg tiles: Triple<String, AppSymbol, Pair<Boolean, () -> Unit>>) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        tiles.forEach { (label, symbol, action) -> ActionTile(label, symbol, Modifier.weight(1f), action.first, action.second) }
    }
}

/** Empty section: large icon, title, explanation and one main button (concept "Хранилищ пока нет"). */
@Composable
fun EmptyState(icon: DrawableResource, title: String, text: String, button: String? = null, enabled: Boolean = true,
    onClick: (() -> Unit)? = null) {
    Column(Modifier.fillMaxWidth().padding(vertical = 24.dp, horizontal = 8.dp), horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(10.dp)) {
        BrandIcon(icon, size = 120.dp)
        Text(title, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
        Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
        if (button != null && onClick != null) Button(onClick = onClick, enabled = enabled, shape = RoundedCornerShape(24.dp),
            colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.primaryContainer,
                contentColor = MaterialTheme.colorScheme.onPrimaryContainer)) {
            AppIcon(AppSymbol.Add); Spacer(Modifier.width(8.dp)); Text(button)
        }
    }
}

/** Rounded search field like the concept's top search. */
@Composable
fun PillSearchField(value: String, onValueChange: (String) -> Unit, placeholder: String, modifier: Modifier = Modifier) {
    val scheme = MaterialTheme.colorScheme
    TextField(value, onValueChange, modifier.fillMaxWidth(), singleLine = true,
        placeholder = { Text(placeholder) },
        leadingIcon = { AppIcon(AppSymbol.Search) },
        trailingIcon = { if (value.isNotEmpty()) IconButton(onClick = { onValueChange("") },
            modifier = Modifier.actionDescription(tr("Очистить", "Clear"))) { AppIcon(AppSymbol.Close) } },
        shape = RoundedCornerShape(28.dp), keyboardOptions = KeyboardOptions.Default,
        colors = TextFieldDefaults.colors(focusedContainerColor = scheme.surfaceContainerHigh, unfocusedContainerColor = scheme.surfaceContainerHigh,
            focusedIndicatorColor = androidx.compose.ui.graphics.Color.Transparent,
            unfocusedIndicatorColor = androidx.compose.ui.graphics.Color.Transparent))
}

