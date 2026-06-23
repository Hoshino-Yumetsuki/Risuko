package app.risuko.webview_upgrade.swap

import android.content.Context
import android.content.Intent
import java.lang.reflect.Method

internal object BindServiceArgsAdapter {

    fun findBestBindServiceMethod(originalAm: Any, srcArgs: Array<Any?>?): Method? {
        val srcLen = srcArgs?.size ?: 0
        val candidates = ArrayList<Method>()
        try {
            val amClass = originalAm.javaClass
            for (iface in amClass.interfaces) {
                for (m in iface.declaredMethods) {
                    if (m.name == "bindService") candidates.add(m)
                }
            }
            if (candidates.isEmpty()) {
                for (m in amClass.methods) {
                    if (m.name == "bindService") candidates.add(m)
                }
            }
        } catch (_: Throwable) {
            return null
        }
        if (candidates.isEmpty()) return null

        var best: Method? = null
        var bestScore = Int.MIN_VALUE
        for (m in candidates) {
            val pc = m.parameterCount
            if (pc > srcLen) continue
            val gap = srcLen - pc
            if (gap > 3) continue
            var score = scorePrefixMatch(m.parameterTypes, srcArgs)
            if (gap <= 2) score += 20
            if (gap == 0) score += 5
            if (score > bestScore) {
                bestScore = score
                best = m
            }
        }
        return best ?: candidates[0]
    }

    private fun scorePrefixMatch(targetTypes: Array<Class<*>>, srcArgs: Array<Any?>?): Int {
        if (srcArgs == null) return 0
        var score = 0
        val n = minOf(targetTypes.size, srcArgs.size)
        for (i in 0 until n) {
            val arg = srcArgs[i] ?: continue
            val t = targetTypes[i]
            val ac = arg.javaClass
            when {
                t.isAssignableFrom(ac) -> score += 4
                t.isPrimitive -> when {
                    t == Integer.TYPE && arg is Int -> score += 4
                    t == java.lang.Long.TYPE && arg is Long -> score += 4
                    t == java.lang.Boolean.TYPE && arg is Boolean -> score += 4
                }
                (t == Integer::class.javaObjectType || t == java.lang.Long::class.javaObjectType) && arg is Number ->
                    score += 2
            }
        }
        return score
    }

    fun adapt(srcArgs: Array<Any?>?, bindServiceMethod: Method, hostPackageName: String?): Array<Any?> {
        val targetParamCount = bindServiceMethod.parameterCount
        val targetTypes = bindServiceMethod.parameterTypes
        val mutable = ArrayList<Any?>()
        if (srcArgs != null) mutable.addAll(srcArgs)

        if (!trimToTargetCount(mutable, targetParamCount, hostPackageName)) {
            legacyTrimToTargetCount(mutable, targetParamCount, hostPackageName)
        }

        val result = arrayOfNulls<Any?>(targetParamCount)
        for (i in 0 until targetParamCount) {
            val value = if (i < mutable.size) mutable[i] else null
            result[i] = coerceValueForType(value, targetTypes[i])
        }
        clearExternalServiceFlag(result)
        return result
    }

    private fun trimToTargetCount(mutable: MutableList<Any?>, targetCount: Int, hostPackageName: String?): Boolean {
        while (mutable.size > targetCount) {
            val diff = mutable.size - targetCount
            val intentIdx = indexOfFirstIntent(mutable)
            if (intentIdx < 0) return false
            val flagsIdx = findFirstFlagsIndexAfter(mutable, intentIdx)
            val userIdIdx = findLastIntegerIndex(mutable)
            if (diff == 1 && flagsIdx > intentIdx && userIdIdx > flagsIdx) {
                val remove = pickExtraSlotBetween(mutable, flagsIdx, userIdIdx, hostPackageName)
                if (remove >= 0) mutable.removeAt(remove) else return false
            } else {
                return false
            }
        }
        return mutable.size == targetCount
    }

    private fun legacyTrimToTargetCount(mutable: MutableList<Any?>, targetCount: Int, hostPackageName: String?) {
        while (mutable.size > targetCount) {
            var removeIndex = findInstanceNameIndexLegacy(mutable, hostPackageName)
            if (removeIndex < 0) removeIndex = findNullableGapIndexLegacy(mutable)
            if (removeIndex < 0) {
                val flagsIndex = findFirstFlagsIndexWholeList(mutable)
                removeIndex = if (flagsIndex >= 0 && flagsIndex + 1 < mutable.size) flagsIndex + 1 else mutable.size - 1
            }
            mutable.removeAt(removeIndex)
        }
    }

    private fun indexOfFirstIntent(args: List<Any?>): Int = args.indexOfFirst { it is Intent }

    private fun findFirstFlagsIndexAfter(args: List<Any?>, intentIdx: Int): Int {
        val start = if (intentIdx >= 0) intentIdx + 1 else 0
        for (i in start until args.size) {
            if (args[i] is Int || args[i] is Long) return i
        }
        return -1
    }

    private fun findFirstFlagsIndexWholeList(args: List<Any?>): Int {
        for (i in args.indices) if (args[i] is Int || args[i] is Long) return i
        return -1
    }

    private fun findLastIntegerIndex(args: List<Any?>): Int {
        for (i in args.indices.reversed()) if (args[i] is Int) return i
        return -1
    }

    private fun pickExtraSlotBetween(args: List<Any?>, flagsIdx: Int, userIdIdx: Int, hostPackageName: String?): Int {
        for (i in flagsIdx + 1 until userIdIdx) {
            val v = args[i]
            if (v == null) return i
            if (v is String && (hostPackageName == null || hostPackageName != v)) return i
        }
        for (i in flagsIdx + 1 until userIdIdx) if (args[i] is String) return i
        return -1
    }

    private fun findInstanceNameIndexLegacy(args: List<Any?>, hostPackageName: String?): Int {
        val flagsIndex = findFirstFlagsIndexWholeList(args)
        val userIdIndex = findLastIntegerIndex(args)
        if (flagsIndex < 0 || userIdIndex <= flagsIndex) return -1
        for (i in flagsIndex + 1 until userIdIndex) {
            val value = args[i] as? String ?: continue
            if (hostPackageName != null && value == hostPackageName) continue
            return i
        }
        return -1
    }

    private fun findNullableGapIndexLegacy(args: List<Any?>): Int {
        val flagsIndex = findFirstFlagsIndexWholeList(args)
        val userIdIndex = findLastIntegerIndex(args)
        if (flagsIndex < 0 || userIdIndex <= flagsIndex) return -1
        for (i in flagsIndex + 1 until userIdIndex) if (args[i] == null) return i
        return -1
    }

    private fun coerceValueForType(value: Any?, targetType: Class<*>): Any? = when (targetType) {
        java.lang.Long.TYPE, java.lang.Long::class.javaObjectType ->
            (value as? Number)?.toLong() ?: 0L
        Integer.TYPE, Integer::class.javaObjectType ->
            (value as? Number)?.toInt() ?: 0
        java.lang.Boolean.TYPE, java.lang.Boolean::class.javaObjectType ->
            value as? Boolean ?: false
        else -> value
    }

    private fun clearExternalServiceFlag(args: Array<Any?>) {
        var mask = 0x00800000L or 0x80000000L
        try {
            mask = mask or Context.BIND_EXTERNAL_SERVICE.toLong()
        } catch (_: Throwable) {
        }
        for (i in args.indices) {
            when (val v = args[i]) {
                is Int -> {
                    val m = (mask and 0xFFFFFFFFL).toInt()
                    args[i] = v and m.inv()
                    return
                }
                is Long -> {
                    args[i] = v and mask.inv()
                    return
                }
            }
        }
    }
}
