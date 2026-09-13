package io.github.gu1p.nodeharbor

import android.net.DnsResolver
import android.os.CancellationSignal
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.junit.Assert.*
import org.junit.Test

class SystemDnsContract {
    @Test fun androidResolvesRawGuestQueriesOnItsDefaultNetwork() {
        val query = byteArrayOf(0x4e, 0x48, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 7) +
            "example".toByteArray() + byteArrayOf(3) + "com".toByteArray() + byteArrayOf(0, 0, 1, 0, 1)
        val done = CountDownLatch(1)
        var result = "No response from Android's default resolver"
        val executor = Executors.newSingleThreadExecutor()
        val cancellation = CancellationSignal()
        try {
            DnsResolver.getInstance().rawQuery(null, query, DnsResolver.FLAG_EMPTY, executor, cancellation,
                object : DnsResolver.Callback<ByteArray> {
                    override fun onAnswer(answer: ByteArray, rcode: Int) {
                        result = if (answer.size >= 12 && rcode == 0) "ok" else "Invalid DNS answer ($rcode)"
                        done.countDown()
                    }
                    override fun onError(error: DnsResolver.DnsException) {
                        result = "DNS error ${error.code}, ${error.cause?.javaClass?.simpleName}"
                        done.countDown()
                    }
                })
            done.await(30, TimeUnit.SECONDS)
            assertEquals(result, "ok", result)
        } finally { cancellation.cancel(); executor.shutdownNow() }
    }
}
