<?php
declare(strict_types=1);
require __DIR__ . "/src/Converter.php";
$converter = new \Octri\Convert\Converter($argv[1]);
if (($argv[2] ?? null) === '--raw') {
    echo $converter->executeJson(stream_get_contents(STDIN));
    exit(0);
}
$input = json_decode(
    stream_get_contents(STDIN),
    false,
    512,
    JSON_THROW_ON_ERROR,
);
if ($input->abiVersion !== 1 || !in_array($input->action, ["convert", "review", "bundle", "compare", "export", "formats", "detect", "batch"], true)) {
    echo $converter->callJson($input);
} else {
    $value = match ($input->action) {
        "convert" => $converter->convert($input->request),
        "review" => $converter->review($input->request),
        "bundle" => $converter->bundle($input->request),
        "detect" => $converter->detect($input->request),
        "batch" => $converter->batch($input->requests),
        "compare" => $converter->compare($input->before, $input->after),
        "export" => $converter->export($input->report, $input->format),
        "formats" => $converter->formats(),
    };
    echo json_encode((object) ["Ok" => $value], JSON_THROW_ON_ERROR);
}
