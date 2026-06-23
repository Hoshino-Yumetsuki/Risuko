package app.risuko.webview_upgrade.reflect

import java.lang.reflect.Field
import java.lang.reflect.Method
import java.lang.reflect.Modifier
import java.lang.reflect.Proxy
import java.util.concurrent.ConcurrentHashMap

internal object Reflect {

    private val classCache = ConcurrentHashMap<String, Class<*>>()
    private val fieldCache = ConcurrentHashMap<String, Field>()
    private val methodCache = ConcurrentHashMap<String, Method>()

    fun cls(name: String): Class<*> =
        classCache.getOrPut(name) { Class.forName(name) }

    fun clsOrNull(name: String): Class<*>? =
        try { cls(name) } catch (t: Throwable) { null }


    private fun findField(start: Class<*>, name: String): Field {
        val key = "${start.name}#$name"
        fieldCache[key]?.let { return it }
        var c: Class<*>? = start
        while (c != null) {
            try {
                val f = c.getDeclaredField(name)
                f.isAccessible = true
                fieldCache[key] = f
                return f
            } catch (_: NoSuchFieldException) {
                c = c.superclass
            }
        }
        throw NoSuchFieldException("$name on ${start.name}")
    }

    fun getStaticField(clazz: Class<*>, name: String): Any? =
        findField(clazz, name).get(null)

    fun setStaticField(clazz: Class<*>, name: String, value: Any?) {
        findField(clazz, name).set(null, value)
    }

    fun getField(target: Any, name: String): Any? =
        findField(target.javaClass, name).get(target)

    fun setField(target: Any, name: String, value: Any?) {
        findField(target.javaClass, name).set(target, value)
    }

    fun setFieldOn(declaringClass: Class<*>, target: Any?, name: String, value: Any?) {
        findField(declaringClass, name).set(target, value)
    }

    private fun argTypeKey(args: Array<out Any?>): String =
        args.joinToString(",") { it?.javaClass?.name ?: "null" }

    private fun findMethod(start: Class<*>, name: String, args: Array<out Any?>): Method {
        val key = "${start.name}#$name(${argTypeKey(args)})"
        methodCache[key]?.let { return it }
        var best: Method? = null
        var bestScore = Int.MIN_VALUE
        var c: Class<*>? = start
        val seen = HashSet<String>()
        while (c != null) {
            for (m in c.declaredMethods) {
                if (m.name != name) continue
                val sig = m.parameterTypes.joinToString(",") { it.name }
                if (!seen.add("$name($sig)")) continue
                val score = scoreParams(m.parameterTypes, args) ?: continue
                if (score > bestScore) {
                    bestScore = score
                    best = m
                }
            }
            c = c.superclass
        }
        val chosen = best ?: throw NoSuchMethodException("$name on ${start.name} for ${argTypeKey(args)}")
        chosen.isAccessible = true
        methodCache[key] = chosen
        return chosen
    }

    private fun scoreParams(types: Array<Class<*>>, args: Array<out Any?>): Int? {
        if (types.size != args.size) return null
        var score = 0
        for (i in types.indices) {
            val t = types[i]
            val a = args[i]
            if (a == null) {
                if (t.isPrimitive) return null
                continue
            }
            val ac = a.javaClass
            when {
                t == ac -> score += 4
                t.isAssignableFrom(ac) -> score += 3
                t.isPrimitive && isBoxedMatch(t, ac) -> score += 4
                else -> return null
            }
        }
        return score
    }

    private fun isBoxedMatch(prim: Class<*>, boxed: Class<*>): Boolean = when (prim) {
        Integer.TYPE -> boxed == Integer::class.javaObjectType
        java.lang.Long.TYPE -> boxed == java.lang.Long::class.javaObjectType
        java.lang.Boolean.TYPE -> boxed == java.lang.Boolean::class.javaObjectType
        java.lang.Short.TYPE -> boxed == java.lang.Short::class.javaObjectType
        java.lang.Byte.TYPE -> boxed == java.lang.Byte::class.javaObjectType
        Character.TYPE -> boxed == Character::class.javaObjectType
        java.lang.Float.TYPE -> boxed == java.lang.Float::class.javaObjectType
        java.lang.Double.TYPE -> boxed == java.lang.Double::class.javaObjectType
        else -> false
    }

    fun callStatic(clazz: Class<*>, name: String, vararg args: Any?): Any? {
        val m = findMethod(clazz, name, args)
        require(Modifier.isStatic(m.modifiers)) { "$name on ${clazz.name} is not static" }
        return m.invoke(null, *args)
    }

    fun call(target: Any, name: String, vararg args: Any?): Any? =
        findMethod(target.javaClass, name, args).invoke(target, *args)

    fun callStaticTyped(
        clazz: Class<*>,
        name: String,
        paramTypes: Array<Class<*>>,
        args: Array<Any?>,
    ): Any? {
        val m = clazz.getDeclaredMethod(name, *paramTypes)
        m.isAccessible = true
        return m.invoke(null, *args)
    }

    fun newInterfaceProxy(
        target: Any,
        interceptors: Map<String, Interceptor>,
    ): Any {
        val interfaces = collectInterfaces(target.javaClass)
        require(interfaces.isNotEmpty()) { "${target.javaClass} implements no interfaces" }
        val loader = interfaces[0].classLoader ?: target.javaClass.classLoader
        return Proxy.newProxyInstance(loader, interfaces) { proxy, method, rawArgs ->
            val args = rawArgs ?: emptyArray()
            when (method.name) {
                "hashCode" -> return@newProxyInstance System.identityHashCode(proxy)
                "equals" -> return@newProxyInstance proxy === args.getOrNull(0)
                "toString" -> return@newProxyInstance "Proxy(${target.javaClass.name})"
            }
            val interceptor = interceptors[method.name]
            if (interceptor != null) {
                interceptor.invoke(args) { method.invoke(target, *args) }
            } else {
                method.invoke(target, *args)
            }
        }
    }

    private fun collectInterfaces(start: Class<*>): Array<Class<*>> {
        val out = LinkedHashSet<Class<*>>()
        var c: Class<*>? = start
        while (c != null && c != Any::class.java) {
            for (i in c.interfaces) addInterface(i, out)
            c = c.superclass
        }
        return out.toTypedArray()
    }

    private fun addInterface(i: Class<*>, out: LinkedHashSet<Class<*>>) {
        if (out.add(i)) {
            for (p in i.interfaces) addInterface(p, out)
        }
    }

    fun interface Interceptor {
        fun invoke(args: Array<Any?>, original: () -> Any?): Any?
    }
}
