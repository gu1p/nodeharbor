package io.github.gu1p.nodeharbor

/** Never initialize this object in the credential-owning application process. */
internal object NativeRuntime {
    init { System.loadLibrary("nodeharbor") }
    external fun version(): String
    external fun run(arguments: Array<String>)
    external fun redirectOutput(descriptor: Int)
}
