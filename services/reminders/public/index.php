<?php
declare(strict_types=1);

$dataDir  = getenv('DATA_DIR') ?: '/data';
$filePath = $dataDir . '/reminders.json';

header('Content-Type: application/json');

function loadReminders(string $path): array {
    if (!file_exists($path)) {
        return [];
    }
    $raw = file_get_contents($path);
    if ($raw === false || trim($raw) === '') {
        return [];
    }
    $decoded = json_decode($raw, true);
    return is_array($decoded) ? $decoded : [];
}

function saveReminders(string $path, array $reminders): void {
    $dir = dirname($path);
    if (!is_dir($dir)) {
        mkdir($dir, 0755, true);
    }
    file_put_contents($path, json_encode($reminders, JSON_PRETTY_PRINT));
}

$method = $_SERVER['REQUEST_METHOD'];
$uri    = parse_url($_SERVER['REQUEST_URI'], PHP_URL_PATH);

// POST /reminders  — add a reminder
if ($method === 'POST' && $uri === '/reminders') {
    $body = json_decode(file_get_contents('php://input'), true) ?? [];
    $userId  = (string)($body['user_id']  ?? '');
    $message = (string)($body['message']  ?? '');
    $duets   = (float) ($body['due_ts']   ?? 0.0);

    if ($userId === '' || $message === '' || $duets <= 0) {
        http_response_code(400);
        echo json_encode(['error' => 'user_id, message, and due_ts are required']);
        exit;
    }

    $reminders = loadReminders($filePath);
    $reminders[] = [
        'user_id' => $userId,
        'message' => $message,
        'due_ts'  => $duets,
    ];
    saveReminders($filePath, $reminders);
    echo json_encode(['ok' => true]);
    exit;
}

// GET /reminders/due  — return and remove due reminders
if ($method === 'GET' && $uri === '/reminders/due') {
    $now       = microtime(true);
    $all       = loadReminders($filePath);
    $due       = array_values(array_filter($all, fn($r) => ($r['due_ts'] ?? 0) <= $now));
    $remaining = array_values(array_filter($all, fn($r) => ($r['due_ts'] ?? 0) > $now));

    if (!empty($due)) {
        saveReminders($filePath, $remaining);
    }

    echo json_encode(['due' => $due]);
    exit;
}

// GET /reminders  — list all pending reminders
if ($method === 'GET' && $uri === '/reminders') {
    echo json_encode(['reminders' => loadReminders($filePath)]);
    exit;
}

// DELETE /reminders/:user_id  — remove all reminders for a user
if ($method === 'DELETE' && str_starts_with($uri, '/reminders/')) {
    $uid = substr($uri, strlen('/reminders/'));
    if ($uid !== '' && $uid !== 'due') {
        $all = loadReminders($filePath);
        $remaining = array_values(array_filter($all, fn($r) => ($r['user_id'] ?? '') !== $uid));
        saveReminders($filePath, $remaining);
        echo json_encode(['ok' => true]);
        exit;
    }
}

http_response_code(404);
echo json_encode(['error' => 'not found']);
