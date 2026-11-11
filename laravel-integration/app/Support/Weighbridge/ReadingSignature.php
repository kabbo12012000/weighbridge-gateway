<?php

namespace App\Support\Weighbridge;

/**
 * Verifies the HMAC the gateway attaches to every weight reading, so the weight saved is
 * exactly what the indicator sent (it cannot be typed or edited in the browser).
 *
 * Must match WeightMsg::signing_payload() in the gateway (src-tauri/src/protocol.rs):
 *   device_id|reading_id|weight with 3 decimals|unit|1 or 0|timestamp (verbatim string)
 */
final class ReadingSignature
{
    public static function payload(array $reading): string
    {
        return implode('|', [
            (string) $reading['device_id'],
            (string) $reading['reading_id'],
            number_format((float) $reading['weight'], 3, '.', ''),
            (string) $reading['unit'],
            filter_var($reading['stable'], FILTER_VALIDATE_BOOLEAN) ? '1' : '0',
            (string) $reading['measured_at'],
        ]);
    }

    public static function valid(array $reading, string $secret): bool
    {
        $expected = hash_hmac('sha256', self::payload($reading), $secret);

        return hash_equals($expected, strtolower((string) $reading['signature']));
    }
}
