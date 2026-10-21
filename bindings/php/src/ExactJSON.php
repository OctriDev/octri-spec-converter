<?php
declare(strict_types=1);

namespace Octri\Convert;

final class LosslessNumberException extends \RangeException
{
    public readonly string $bindingCode;
    public function __construct()
    {
        $this->bindingCode = 'LOSSLESS_NUMBER_REQUIRED';
        parent::__construct('Decoded PHP values would change an exact JSON number. Use executeJson or callJson.');
    }
}

/** Transport guard. Never rounds a source number into PHP's float model. */
final class ExactJSON
{
    private static function decimal(string $token): ?string
    {
        $parts = preg_split('/[eE]/', $token);
        $mantissa = $parts[0];
        $exponent = $parts[1] ?? '0';
        $negative = str_starts_with($mantissa, '-');
        $unsigned = $negative ? substr($mantissa, 1) : $mantissa;
        $dot = strpos($unsigned, '.');
        $digits = ltrim(str_replace('.', '', $unsigned), '0');
        if ($digits === '') return '0';
        if (strlen(ltrim($exponent, '+-0')) > 15) return null;
        $power = (int) $exponent - ($dot === false ? 0 : strlen($unsigned) - $dot - 1);
        $coefficient = rtrim($digits, '0');
        $power += strlen($digits) - strlen($coefficient);
        return ($negative ? '-' : '') . $coefficient . 'e' . $power;
    }

    public static function decode(string $raw): \stdClass
    {
        preg_match_all('~"(?:[^"\\\\]|\\\\.)*"|-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?~s', $raw, $tokens);
        foreach ($tokens[0] as $token) {
            if ($token[0] === '"') continue;
            if (strpbrk($token, '.eE') !== false) {
                $value = (float) $token;
                if (!is_finite($value) || self::decimal($token) === null
                    || self::decimal($token) !== self::decimal(json_encode($value, JSON_THROW_ON_ERROR))) {
                    throw new LosslessNumberException();
                }
            } else {
                $negative = $token[0] === '-';
                $digits = $negative ? substr($token, 1) : $token;
                $bound = $negative ? substr((string) PHP_INT_MIN, 1) : (string) PHP_INT_MAX;
                if (strlen($digits) > strlen($bound)
                    || (strlen($digits) === strlen($bound) && strcmp($digits, $bound) > 0)) {
                    throw new LosslessNumberException();
                }
            }
        }
        return json_decode($raw, false, 512, JSON_THROW_ON_ERROR);
    }
}
