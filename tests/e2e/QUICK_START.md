# E2E Tests - Quick Start Guide

## 🚀 Get Started in 5 Minutes

### 1. Prerequisites Check
```bash
# Verify gateway is running
curl http://192.168.122.173:9000/health

# Check Node.js
node --version  # Should be v18+
```

### 2. Setup
```bash
cd tests/e2e
./setup-test-env.sh
```

This will:
- ✅ Check gateway connectivity
- ✅ Install npm dependencies
- ✅ Install Playwright browsers
- ✅ Verify test environment

### 3. Run Tests
```bash
./run-tests.sh
```

## 📊 Common Commands

### Run All Tests
```bash
npm test
```

### Run Specific Suite
```bash
npm test -- tests/01-registration.spec.ts
npm test -- tests/02-login.spec.ts
npm test -- tests/09-cross-platform.spec.ts
```

### Interactive Debugging
```bash
npm run test:ui          # Playwright UI mode (best for debugging)
npm run test:headed      # Watch tests run in browser
npm run test:debug       # Step-by-step debugging
```

### View Reports
```bash
npm run test:report      # Open HTML report
```

## 🎯 Test Suites Overview

| File | Tests | Description |
|------|-------|-------------|
| `01-registration.spec.ts` | 7 | User registration, validation |
| `02-login.spec.ts` | 8 | Login flows, tenant selection |
| `03-tenant-switching.spec.ts` | 6 | Multi-tenant operations |
| `04-oauth.spec.ts` | 2 | OAuth flows (partially manual) |
| `05-token-refresh.spec.ts` | 5 | Token lifecycle management |
| `06-password-reset.spec.ts` | 4 | Password reset flows |
| `07-invitations.spec.ts` | 2 | User invitations (partially manual) |
| `08-rbac.spec.ts` | 3 | Role-based access control |
| `09-cross-platform.spec.ts` | 7 | Cross-platform identity |

## ⚠️ Important Notes

### Required Test Tenants
Before running tests, ensure these tenants exist:
- `progalaxy/test-tenant`
- `btechrecruiter/test-company`

### Gateway URL
Default: `http://192.168.122.173:9000`

To use a different URL:
```bash
export GATEWAY_URL=http://your-gateway:9000
npm test
```

Or edit `.env` file.

## 🐛 Troubleshooting

### Gateway Not Accessible
```bash
# Check service
sudo systemctl status stonescriptdb-gateway

# Check connectivity
ping 192.168.122.173
curl http://192.168.122.173:9000/health
```

### Tests Failing
```bash
# Run in headed mode to see what's happening
npm run test:headed

# Or use UI mode for interactive debugging
npm run test:ui
```

### Clean Reinstall
```bash
rm -rf node_modules package-lock.json
npm install
npx playwright install
```

## 📈 CI Integration

Tests run automatically on:
- ✅ Push to `main` or `develop`
- ✅ Pull requests
- ✅ Manual workflow dispatch

View results in GitHub Actions tab.

## 🎓 Next Steps

1. **Read Full Docs**: See `README.md` for comprehensive guide
2. **Review Test Code**: Check `tests/` directory for examples
3. **Add New Tests**: Use existing tests as templates
4. **Check Summary**: See `E2E_TESTS_SUMMARY.md` for detailed coverage

## 💡 Tips

- Use `test.only()` to run a single test while debugging
- Use `test.skip()` to temporarily disable tests
- Check `playwright-report/` for detailed failure info
- Videos and screenshots saved on test failures
- Test data uses unique emails - no cleanup needed

## 🆘 Need Help?

1. Check `README.md` for detailed documentation
2. Review `E2E_TESTS_SUMMARY.md` for implementation details
3. Open an issue if you find bugs
4. Check Playwright docs: https://playwright.dev
